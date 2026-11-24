//! Incremental decoder for AWS's `application/vnd.amazon.eventstream` framing.
//!
//! `bedrock-runtime`'s `InvokeModelWithResponseStream` does not return SSE: it
//! returns a sequence of binary frames. Each frame is
//!
//! ```text
//! +---------------------------+
//! | total_length      u32 BE  |  entire frame, including the trailing CRC
//! | headers_length    u32 BE  |
//! | prelude_crc       u32 BE  |  CRC32 of the preceding 8 bytes
//! +---------------------------+
//! | headers   headers_length  |
//! +---------------------------+
//! | payload                   |  total_length - 16 - headers_length
//! +---------------------------+
//! | message_crc       u32 BE  |  CRC32 of every byte before it
//! +---------------------------+
//! ```
//!
//! and each header is `name_len:u8`, `name`, `value_type:u8`, then a
//! type-dependent value.
//!
//! This module owns *only* the framing. It hands back the raw payload bytes plus
//! the reserved `:`-prefixed headers that classify the frame; interpreting the
//! payload is the caller's job (see [`crate::endpoint::bedrock_messages`]).
//!
//! Both CRCs are verified. A corrupt frame is reported as an error rather than
//! decoded on the assumption that the length fields are trustworthy — a bad
//! `total_length` would otherwise desynchronize every subsequent frame.
//!
//! The framing description above is derived from AWS's published event-stream
//! documentation, not from a recorded live `bedrock-runtime` response.

use std::collections::HashMap;

use thiserror::Error;

/// Bytes before the headers: three big-endian `u32`s.
const PRELUDE_LEN: usize = 12;
/// Bytes covered by the prelude CRC: `total_length` and `headers_length`.
const PRELUDE_CRC_INPUT_LEN: usize = 8;
/// Trailing message CRC.
const MESSAGE_CRC_LEN: usize = 4;
/// Smallest legal frame: prelude plus message CRC, no headers and no payload.
const MIN_FRAME_LEN: usize = PRELUDE_LEN + MESSAGE_CRC_LEN;
/// Upper bound on a single frame, matching AWS's documented 16 MiB message
/// limit. A larger `total_length` is treated as corruption instead of being
/// used to size an allocation.
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Reserved header naming the frame class (`event`, `exception`, `error`).
const MESSAGE_TYPE_HEADER: &str = ":message-type";
/// Reserved header naming the event, present on `event` frames.
const EVENT_TYPE_HEADER: &str = ":event-type";
/// Reserved header naming the modeled exception, present on `exception` frames.
const EXCEPTION_TYPE_HEADER: &str = ":exception-type";
/// Reserved header carrying a non-modeled error code.
const ERROR_CODE_HEADER: &str = ":error-code";
/// Reserved header carrying a non-modeled error message.
const ERROR_MESSAGE_HEADER: &str = ":error-message";

/// Reason a frame could not be decoded.
///
/// Every variant is fatal for the stream: once framing integrity is in doubt,
/// the byte offset of the next frame is unknown.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum EventStreamError {
    #[error(
        "event stream frame declares total_length {total_length} \
(headers_length {headers_length}), which is not a valid frame layout"
    )]
    InvalidFrameLength {
        total_length: usize,
        headers_length: usize,
    },
    #[error(
        "event stream frame prelude CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}"
    )]
    PreludeCrcMismatch { expected: u32, actual: u32 },
    #[error("event stream frame CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}")]
    MessageCrcMismatch { expected: u32, actual: u32 },
    #[error("event stream frame has a malformed header block: {0}")]
    MalformedHeaders(String),
}

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventStreamFrame {
    /// `:message-type`, when present.
    pub(crate) message_type: Option<String>,
    /// `:event-type`, when present.
    pub(crate) event_type: Option<String>,
    /// `:exception-type`, when present.
    pub(crate) exception_type: Option<String>,
    /// `:error-code`, when present.
    pub(crate) error_code: Option<String>,
    /// `:error-message`, when present.
    pub(crate) error_message: Option<String>,
    /// Raw payload bytes; interpretation is the caller's.
    pub(crate) payload: Vec<u8>,
}

impl EventStreamFrame {
    /// Whether this frame carries a modeled event rather than a failure.
    ///
    /// `:message-type` is treated as `event` when absent: the header is
    /// required by the protocol, and refusing to decode a payload because a
    /// classification header is missing would be more brittle than assuming the
    /// common case, given failures are also identifiable by their own
    /// `:exception-type`/`:error-code` headers.
    pub(crate) fn is_event(&self) -> bool {
        match self.message_type.as_deref() {
            Some("event") | None => true,
            Some(_) => false,
        }
    }

    /// Best-available label for a failure frame.
    pub(crate) fn failure_kind(&self) -> Option<&str> {
        self.exception_type
            .as_deref()
            .or(self.error_code.as_deref())
            .or(self.message_type.as_deref())
    }
}

/// Buffers bytes until whole frames are available.
///
/// Frames routinely span byte-chunk boundaries and several frames routinely
/// arrive in one chunk, so the decoder keeps its own buffer and yields frames
/// one at a time.
#[derive(Debug, Default)]
pub(crate) struct EventStreamDecoder {
    buffer: Vec<u8>,
}

impl EventStreamDecoder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Appends transport bytes.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Whether any bytes remain buffered.
    ///
    /// After the transport closes this distinguishes a clean end from a
    /// truncated trailing frame.
    pub(crate) fn has_buffered_bytes(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Decodes the next complete frame.
    ///
    /// Returns `Ok(None)` when the buffer does not yet hold a whole frame.
    pub(crate) fn next_frame(&mut self) -> Result<Option<EventStreamFrame>, EventStreamError> {
        if self.buffer.len() < PRELUDE_LEN {
            return Ok(None);
        }

        let total_length = read_u32(&self.buffer, 0) as usize;
        let headers_length = read_u32(&self.buffer, 4) as usize;
        let declared_prelude_crc = read_u32(&self.buffer, 8);

        // Validate the prelude CRC before trusting either length: the lengths
        // decide how many bytes to consume, so acting on a corrupt prelude
        // would lose frame alignment permanently.
        let actual_prelude_crc = crc32(&self.buffer[..PRELUDE_CRC_INPUT_LEN]);
        if actual_prelude_crc != declared_prelude_crc {
            return Err(EventStreamError::PreludeCrcMismatch {
                expected: declared_prelude_crc,
                actual: actual_prelude_crc,
            });
        }

        if !(MIN_FRAME_LEN..=MAX_FRAME_LEN).contains(&total_length)
            || headers_length > total_length - MIN_FRAME_LEN
        {
            return Err(EventStreamError::InvalidFrameLength {
                total_length,
                headers_length,
            });
        }

        if self.buffer.len() < total_length {
            return Ok(None);
        }

        let frame = &self.buffer[..total_length];
        let crc_offset = total_length - MESSAGE_CRC_LEN;
        let declared_message_crc = read_u32(frame, crc_offset);
        let actual_message_crc = crc32(&frame[..crc_offset]);
        if actual_message_crc != declared_message_crc {
            return Err(EventStreamError::MessageCrcMismatch {
                expected: declared_message_crc,
                actual: actual_message_crc,
            });
        }

        let headers_end = PRELUDE_LEN + headers_length;
        let headers = parse_headers(&frame[PRELUDE_LEN..headers_end])?;
        let payload = frame[headers_end..crc_offset].to_vec();
        self.buffer.drain(..total_length);

        Ok(Some(EventStreamFrame {
            message_type: headers.get(MESSAGE_TYPE_HEADER).cloned(),
            event_type: headers.get(EVENT_TYPE_HEADER).cloned(),
            exception_type: headers.get(EXCEPTION_TYPE_HEADER).cloned(),
            error_code: headers.get(ERROR_CODE_HEADER).cloned(),
            error_message: headers.get(ERROR_MESSAGE_HEADER).cloned(),
            payload,
        }))
    }
}

/// Reads a big-endian `u32` at `offset`.
///
/// Callers must have length-checked `bytes`; a short slice yields 0 rather than
/// panicking, which the surrounding length validation then rejects.
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let Some(slice) = bytes.get(offset..offset + 4) else {
        return 0;
    };
    let mut buf = [0u8; 4];
    buf.copy_from_slice(slice);
    u32::from_be_bytes(buf)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(bytes);
    hasher.finalize()
}

/// Parses the header block into a `name -> string value` map.
///
/// Only string-shaped values are retained, because every header this decoder
/// consumes (`:message-type`, `:event-type`, `:exception-type`,
/// `:error-code`, `:error-message`) is a string. Other value types are still
/// *skipped correctly* — their encoded width is known, so an unrecognized
/// header cannot desynchronize the block — but their values are dropped. An
/// unknown value type has no known width, so it is reported as malformed rather
/// than guessed at.
fn parse_headers(mut bytes: &[u8]) -> Result<HashMap<String, String>, EventStreamError> {
    let mut headers = HashMap::new();
    while !bytes.is_empty() {
        let (name_len, rest) = split_first(bytes, "header name length")?;
        let (name, rest) = split_at(rest, name_len as usize, "header name")?;
        let name = String::from_utf8(name.to_vec())
            .map_err(|e| EventStreamError::MalformedHeaders(format!("header name: {e}")))?;
        let (value_type, rest) = split_first(rest, "header value type")?;

        let rest = match value_type {
            // BOOL_TRUE / BOOL_FALSE carry no value bytes.
            0 => {
                headers.insert(name, "true".to_string());
                rest
            }
            1 => {
                headers.insert(name, "false".to_string());
                rest
            }
            // Fixed-width numeric and uuid values: skipped, not interpreted.
            2 => skip_fixed(rest, 1, &name)?,
            3 => skip_fixed(rest, 2, &name)?,
            4 => skip_fixed(rest, 4, &name)?,
            // LONG and TIMESTAMP are both 8 bytes.
            5 | 8 => skip_fixed(rest, 8, &name)?,
            9 => skip_fixed(rest, 16, &name)?,
            // BYTE_ARRAY and STRING share a 2-byte big-endian length prefix.
            6 | 7 => {
                let (len, rest) = split_at(rest, 2, "header value length")?;
                let len = u16::from_be_bytes([len[0], len[1]]) as usize;
                let (value, rest) = split_at(rest, len, "header value")?;
                if value_type == 7 {
                    let value = String::from_utf8(value.to_vec()).map_err(|e| {
                        EventStreamError::MalformedHeaders(format!("header `{name}` value: {e}"))
                    })?;
                    headers.insert(name, value);
                }
                rest
            }
            other => {
                return Err(EventStreamError::MalformedHeaders(format!(
                    "header `{name}` has unknown value type {other}"
                )));
            }
        };
        bytes = rest;
    }
    Ok(headers)
}

fn split_first<'a>(bytes: &'a [u8], what: &str) -> Result<(u8, &'a [u8]), EventStreamError> {
    bytes
        .split_first()
        .map(|(first, rest)| (*first, rest))
        .ok_or_else(|| EventStreamError::MalformedHeaders(format!("truncated before {what}")))
}

fn split_at<'a>(
    bytes: &'a [u8],
    len: usize,
    what: &str,
) -> Result<(&'a [u8], &'a [u8]), EventStreamError> {
    if bytes.len() < len {
        return Err(EventStreamError::MalformedHeaders(format!(
            "truncated {what}: need {len} bytes, have {}",
            bytes.len()
        )));
    }
    Ok(bytes.split_at(len))
}

fn skip_fixed<'a>(bytes: &'a [u8], len: usize, name: &str) -> Result<&'a [u8], EventStreamError> {
    split_at(bytes, len, &format!("value of header `{name}`")).map(|(_, rest)| rest)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Frame builders shared by this module's tests and the Bedrock endpoint's.

    use super::MESSAGE_CRC_LEN;
    use super::PRELUDE_CRC_INPUT_LEN;
    use super::PRELUDE_LEN;
    use super::crc32;

    /// Encodes one `:`-prefixed string header.
    pub(crate) fn string_header(name: &str, value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        #[allow(clippy::cast_possible_truncation)]
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(7); // STRING
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
        out
    }

    /// Builds a fully valid frame (both CRCs correct) from encoded headers and a
    /// payload.
    pub(crate) fn frame(headers: &[u8], payload: &[u8]) -> Vec<u8> {
        let total_length = PRELUDE_LEN + headers.len() + payload.len() + MESSAGE_CRC_LEN;
        let mut out = Vec::with_capacity(total_length);
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(total_length as u32).to_be_bytes());
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        out.extend_from_slice(&crc32(&out[..PRELUDE_CRC_INPUT_LEN]).to_be_bytes());
        out.extend_from_slice(headers);
        out.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&out).to_be_bytes());
        out
    }

    /// Builds a `:message-type: event` frame carrying `payload`.
    pub(crate) fn event_frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        let mut headers = string_header(":event-type", event_type);
        headers.extend_from_slice(&string_header(":message-type", "event"));
        headers.extend_from_slice(&string_header(":content-type", "application/json"));
        frame(&headers, payload)
    }

    /// Builds a `:message-type: exception` frame.
    pub(crate) fn exception_frame(exception_type: &str, payload: &[u8]) -> Vec<u8> {
        let mut headers = string_header(":exception-type", exception_type);
        headers.extend_from_slice(&string_header(":message-type", "exception"));
        frame(&headers, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::event_frame;
    use super::test_support::exception_frame;
    use super::test_support::frame;
    use super::test_support::string_header;
    use super::*;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;

    /// Known-answer vectors for the reflected IEEE CRC32 the framing uses,
    /// pinning the hash the decoder validates against.
    #[test]
    fn crc32_matches_known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn decodes_an_event_frame_with_headers_and_payload() {
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&event_frame("chunk", br#"{"bytes":"abc"}"#));

        let frame = decoder
            .next_frame()
            .expect("frame should decode")
            .expect("a complete frame");

        assert_eq!(frame.message_type.as_deref(), Some("event"));
        assert_eq!(frame.event_type.as_deref(), Some("chunk"));
        assert_eq!(frame.payload, br#"{"bytes":"abc"}"#.to_vec());
        assert!(frame.is_event());
        assert!(!decoder.has_buffered_bytes());
        assert_eq!(decoder.next_frame().expect("no error"), None);
    }

    #[test]
    fn a_frame_split_across_many_chunks_is_reassembled() {
        let bytes = event_frame("chunk", br#"{"bytes":"split"}"#);
        let mut decoder = EventStreamDecoder::new();

        // One byte at a time: the pathological case for an incremental decoder.
        for (index, byte) in bytes.iter().enumerate() {
            decoder.push(&[*byte]);
            let decoded = decoder.next_frame().expect("partial input is not an error");
            if index + 1 < bytes.len() {
                assert_eq!(
                    decoded, None,
                    "frame must not be emitted before byte {index}"
                );
            } else {
                let frame = decoded.expect("final byte should complete the frame");
                assert_eq!(frame.payload, br#"{"bytes":"split"}"#.to_vec());
            }
        }
    }

    #[test]
    fn back_to_back_frames_in_one_chunk_are_decoded_in_order() {
        let mut bytes = event_frame("chunk", b"first");
        bytes.extend_from_slice(&event_frame("chunk", b"second"));
        bytes.extend_from_slice(&event_frame("chunk", b"third"));
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&bytes);

        let mut payloads = Vec::new();
        while let Some(frame) = decoder.next_frame().expect("frames should decode") {
            payloads.push(String::from_utf8(frame.payload).expect("utf8 payload"));
        }

        assert_eq!(payloads, vec!["first", "second", "third"]);
        assert!(!decoder.has_buffered_bytes());
    }

    #[test]
    fn a_truncated_trailing_frame_is_withheld_and_stays_buffered() {
        let mut bytes = event_frame("chunk", b"complete");
        let partial = event_frame("chunk", b"incomplete");
        bytes.extend_from_slice(&partial[..partial.len() - 3]);
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&bytes);

        let first = decoder
            .next_frame()
            .expect("first frame should decode")
            .expect("a complete frame");
        assert_eq!(first.payload, b"complete".to_vec());

        assert_eq!(
            decoder.next_frame().expect("truncation is not corruption"),
            None
        );
        assert!(
            decoder.has_buffered_bytes(),
            "the partial frame must stay buffered so a close can be reported as truncation"
        );
    }

    #[test]
    fn a_bad_prelude_crc_is_rejected() {
        let mut bytes = event_frame("chunk", b"payload");
        // Corrupt `headers_length`, which the prelude CRC covers.
        bytes[7] ^= 0xFF;
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&bytes);

        assert_matches!(
            decoder.next_frame(),
            Err(EventStreamError::PreludeCrcMismatch { .. })
        );
    }

    #[test]
    fn a_bad_message_crc_is_rejected() {
        let mut bytes = event_frame("chunk", b"payload");
        // Corrupt the payload, leaving the prelude (and its CRC) intact.
        let corrupt_at = bytes.len() - 6;
        bytes[corrupt_at] ^= 0xFF;
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&bytes);

        let error = decoder
            .next_frame()
            .expect_err("a corrupt payload must not be decoded");
        assert_matches!(error, EventStreamError::MessageCrcMismatch { .. });
        assert!(
            error.to_string().contains("CRC32 mismatch"),
            "error should name the failure: {error}"
        );
    }

    #[test]
    fn an_impossible_total_length_is_rejected_rather_than_allocated() {
        // A valid prelude CRC over lengths that cannot describe a frame: the
        // header block alone would overrun the message.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&64u32.to_be_bytes());
        bytes.extend_from_slice(&1_000u32.to_be_bytes());
        bytes.extend_from_slice(&crc32(&bytes).to_be_bytes());
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&bytes);

        assert_matches!(
            decoder.next_frame(),
            Err(EventStreamError::InvalidFrameLength {
                total_length: 64,
                headers_length: 1_000,
            })
        );
    }

    #[test]
    fn an_exception_frame_reports_its_exception_type() {
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&exception_frame(
            "throttlingException",
            br#"{"message":"Too many requests"}"#,
        ));

        let frame = decoder
            .next_frame()
            .expect("frame should decode")
            .expect("a complete frame");

        assert!(!frame.is_event());
        assert_eq!(frame.failure_kind(), Some("throttlingException"));
        assert_eq!(
            frame.payload,
            br#"{"message":"Too many requests"}"#.to_vec()
        );
    }

    /// Encodes `name_len | name | value_type | raw_value` for tests that need
    /// non-string header value types.
    fn typed_header(name: &str, value_type: u8, raw_value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        #[allow(clippy::cast_possible_truncation)]
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(value_type);
        out.extend_from_slice(raw_value);
        out
    }

    #[test]
    fn non_string_header_types_are_skipped_without_losing_frame_alignment() {
        // A BOOL_TRUE, a BYTE, a SHORT, an INTEGER, a LONG, a TIMESTAMP, a
        // BYTE_ARRAY, and a UUID ahead of the headers the decoder needs.
        let mut headers = typed_header("a-bool", 0, b"");
        headers.extend_from_slice(&typed_header("a-byte", 2, &[7]));
        headers.extend_from_slice(&typed_header("a-short", 3, &9i16.to_be_bytes()));
        headers.extend_from_slice(&typed_header("an-int", 4, &11i32.to_be_bytes()));
        headers.extend_from_slice(&typed_header("a-long", 5, &42i64.to_be_bytes()));
        headers.extend_from_slice(&typed_header(
            "a-ts",
            8,
            &1_700_000_000_000i64.to_be_bytes(),
        ));
        let mut byte_array = 3u16.to_be_bytes().to_vec();
        byte_array.extend_from_slice(b"raw");
        headers.extend_from_slice(&typed_header("an-array", 6, &byte_array));
        headers.extend_from_slice(&typed_header("an-uuid", 9, &[0u8; 16]));
        headers.extend_from_slice(&string_header(":message-type", "event"));
        headers.extend_from_slice(&string_header(":event-type", "chunk"));

        let mut decoder = EventStreamDecoder::new();
        decoder.push(&frame(&headers, b"payload"));

        let decoded = decoder
            .next_frame()
            .expect("unknown-but-typed headers must not fail the frame")
            .expect("a complete frame");

        assert_eq!(decoded.event_type.as_deref(), Some("chunk"));
        assert_eq!(decoded.payload, b"payload".to_vec());
    }

    #[test]
    fn an_unknown_header_value_type_is_reported_not_panicked_on() {
        // No such value type: its encoded width is unknowable, so the header
        // block cannot be walked past it.
        let headers = typed_header("mystery", 200, b"whatever");

        let mut decoder = EventStreamDecoder::new();
        decoder.push(&frame(&headers, b"payload"));

        let error = decoder
            .next_frame()
            .expect_err("an unknown header value type must be an error");
        assert_matches!(error, EventStreamError::MalformedHeaders(_));
        assert!(
            error.to_string().contains("unknown value type 200"),
            "error should name the offending type: {error}"
        );
    }

    #[test]
    fn a_truncated_header_block_is_reported_as_malformed() {
        // Declares a 20-byte header name but supplies fewer bytes.
        let mut headers = Vec::new();
        headers.push(20u8);
        headers.extend_from_slice(b"short");

        let mut decoder = EventStreamDecoder::new();
        decoder.push(&frame(&headers, b""));

        assert_matches!(
            decoder.next_frame(),
            Err(EventStreamError::MalformedHeaders(_))
        );
    }

    #[test]
    fn a_frame_without_headers_or_payload_decodes() {
        let mut decoder = EventStreamDecoder::new();
        decoder.push(&frame(b"", b""));

        let decoded = decoder
            .next_frame()
            .expect("an empty frame is legal")
            .expect("a complete frame");

        assert_eq!(decoded.message_type, None);
        assert!(decoded.payload.is_empty());
        // `:message-type` is absent, so the frame is treated as an event.
        assert!(decoded.is_event());
    }
}
