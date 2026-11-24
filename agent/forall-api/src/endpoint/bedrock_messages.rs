use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::messages_common::MessagesApiRequest;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse::bedrock_messages::map_bedrock_http_error;
use crate::sse::spawn_bedrock_messages_stream;
use crate::telemetry::SseTelemetry;
use forall_client::EncodedJsonBody;
use forall_client::HttpTransport;
use forall_client::RequestCompression;
use forall_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use serde_json::Value;
use std::sync::Arc;
use tracing::instrument;

/// API version Bedrock requires *in the body* of an Anthropic invoke request.
///
/// Direct Anthropic takes its version in the `anthropic-version` header; on
/// Bedrock the header does not exist and this value identifies the Bedrock
/// dialect of the Messages schema.
const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
/// Body field naming that version.
const ANTHROPIC_VERSION_FIELD: &str = "anthropic_version";
/// Model is URL-path scoped on Bedrock, so it must not appear in the body.
const MODEL_FIELD: &str = "model";
/// Streaming is selected by the operation (`invoke-with-response-stream`), not
/// by a body flag.
const STREAM_FIELD: &str = "stream";

/// Streaming client for Claude models on Amazon Bedrock
/// (`POST /model/{modelId}/invoke-with-response-stream`).
///
/// The request body is the Anthropic Messages body — this reuses
/// [`MessagesApiRequest`] verbatim rather than defining a parallel wire type —
/// with two Bedrock-specific edits applied by [`bedrock_invoke_body`].
///
/// The response is *not* SSE: it is `application/vnd.amazon.eventstream`. Frame
/// decoding and the hand-off to the shared Messages translator live in
/// [`crate::sse::bedrock_messages`].
///
/// Auth is not handled here: the provider's SigV4 auth provider signs the
/// prepared request, so no credential is constructed in this module.
pub struct BedrockMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct BedrockMessagesOptions {
    pub extra_headers: HeaderMap,
    pub compression: Compression,
    /// Names of tools the request advertised as freeform (see
    /// `forall_tools::freeform_tool_names`). The translator unwraps those tool
    /// calls back into `CustomToolCall`, the only shape core can route to a
    /// freeform handler.
    pub freeform_tool_names: Vec<String>,
}

impl<T: HttpTransport> BedrockMessagesClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    /// Streams one turn.
    ///
    /// `model_id` is a separate parameter rather than a body field because the
    /// Bedrock endpoint is model-scoped: the identifier is part of the URL path.
    #[instrument(
        name = "bedrock_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "bedrock_event_stream",
            http.method = "POST",
            api.path = "model/invoke-with-response-stream"
        )
    )]
    pub async fn stream_request(
        &self,
        model_id: &str,
        request: MessagesApiRequest,
        options: BedrockMessagesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let BedrockMessagesOptions {
            extra_headers,
            compression,
            freeform_tool_names,
        } = options;

        let body = EncodedJsonBody::encode(&bedrock_invoke_body(&request)?).map_err(|e| {
            ApiError::Stream(format!("failed to encode bedrock messages request: {e}"))
        })?;

        // No `session-id`/`thread-id`/`x-openai-subagent` headers here: those
        // are Forall/OpenAI backend routing affordances with no meaning to AWS,
        // and every header sent is covered by the SigV4 signature.
        // `extra_headers` still carries rollout-trace headers supplied by the
        // caller.
        self.stream_encoded(
            model_id,
            body,
            extra_headers,
            compression,
            freeform_tool_names,
        )
        .await
    }

    fn path(model_id: &str) -> String {
        format!(
            "model/{}/invoke-with-response-stream",
            encode_path_segment(model_id)
        )
    }

    async fn stream_encoded(
        &self,
        model_id: &str,
        body: EncodedJsonBody,
        extra_headers: HeaderMap,
        compression: Compression,
        freeform_tool_names: Vec<String>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(
                Method::POST,
                &Self::path(model_id),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("application/vnd.amazon.eventstream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await
            .map_err(map_bedrock_http_error)?;

        Ok(spawn_bedrock_messages_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            freeform_tool_names,
        ))
    }
}

/// Converts an Anthropic Messages body into the Bedrock invoke body.
///
/// Exactly three edits separate the two wires:
///   * `model` is dropped — it is URL-path scoped on Bedrock, and Bedrock's
///     model-body schema rejects extraneous keys;
///   * `stream` is dropped — `invoke-with-response-stream` always streams, so
///     the flag is both redundant and extraneous; and
///   * `anthropic_version` is added, replacing direct Anthropic's
///     `anthropic-version` request header.
///
/// This edits a serialized `Value` rather than introducing a second request
/// struct so that the two wires cannot drift as `MessagesApiRequest` evolves:
/// a new field is picked up automatically instead of needing to be mirrored.
fn bedrock_invoke_body(request: &MessagesApiRequest) -> Result<Value, ApiError> {
    let mut body = serde_json::to_value(request).map_err(|e| {
        ApiError::Stream(format!("failed to serialize bedrock messages request: {e}"))
    })?;
    let Value::Object(map) = &mut body else {
        return Err(ApiError::Stream(
            "messages request did not serialize to a JSON object".to_string(),
        ));
    };
    map.remove(MODEL_FIELD);
    map.remove(STREAM_FIELD);
    map.insert(
        ANTHROPIC_VERSION_FIELD.to_string(),
        Value::String(BEDROCK_ANTHROPIC_VERSION.to_string()),
    );
    Ok(body)
}

/// Percent-encodes one URL path segment, keeping only RFC 3986 unreserved
/// characters literal.
///
/// Bedrock model identifiers are not path-safe as written: on-demand ids end in
/// a version suffix (`...-v1:0`) and an ARN or inference-profile ARN contains
/// `/` and `:`. Leaving those literal would either be rejected or, worse, split
/// the path and change which resource is addressed. `.` and `-` are unreserved,
/// so the common `anthropic.claude-...` and `us.anthropic.claude-...` forms pass
/// through unchanged.
fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages_common::MessagesContentBlock;
    use crate::messages_common::MessagesMessage;
    use crate::messages_common::MessagesRole;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn request(model: &str) -> MessagesApiRequest {
        MessagesApiRequest {
            model: model.to_string(),
            system: None,
            messages: vec![MessagesMessage {
                role: MessagesRole::User,
                content: vec![MessagesContentBlock::Text {
                    text: "hi".to_string(),
                    cache_control: None,
                }],
            }],
            tools: Vec::new(),
            tool_choice: None,
            max_tokens: 1024,
            thinking: None,
            stream: true,
            metadata: None,
        }
    }

    #[test]
    fn invoke_body_drops_model_and_stream_and_adds_anthropic_version() {
        let body =
            bedrock_invoke_body(&request("anthropic.claude-opus-4-6")).expect("body should build");

        assert_eq!(
            body,
            json!({
                "anthropic_version": "bedrock-2023-05-31",
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "hi"}]}
                ],
                "max_tokens": 1024
            })
        );
    }

    #[test]
    fn path_is_model_scoped_and_url_encoded() {
        // On-demand ids are already path-safe; only the version suffix's colon
        // needs encoding.
        assert_eq!(
            BedrockMessagesClient::<forall_client::ReqwestTransport>::path(
                "anthropic.claude-opus-4-6-20260101-v1:0"
            ),
            "model/anthropic.claude-opus-4-6-20260101-v1%3A0/invoke-with-response-stream"
        );
    }

    #[test]
    fn cross_region_inference_profile_ids_pass_through_unchanged() {
        // `.` is unreserved, so a geo-prefixed inference profile id needs no
        // escaping at all.
        assert_eq!(
            BedrockMessagesClient::<forall_client::ReqwestTransport>::path(
                "us.anthropic.claude-opus-4-6"
            ),
            "model/us.anthropic.claude-opus-4-6/invoke-with-response-stream"
        );
    }

    #[test]
    fn arn_model_ids_have_their_separators_encoded() {
        // An inference-profile ARN would otherwise inject extra path segments
        // and address a different resource.
        assert_eq!(
            encode_path_segment(
                "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.anthropic.claude"
            ),
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A123456789012%3Ainference-profile%2Fus.anthropic.claude"
        );
    }
}
