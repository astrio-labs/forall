use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::gemini_common::GeminiRequest;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse::gemini::map_gemini_http_error;
use crate::sse::spawn_gemini_stream;
use crate::telemetry::SseTelemetry;
use forall_client::EncodedJsonBody;
use forall_client::HttpTransport;
use forall_client::RequestCompression;
use forall_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::instrument;

/// Query parameter that switches `streamGenerateContent` from its default
/// (a chunked JSON *array* of `GenerateContentResponse` objects) to a real
/// `text/event-stream`. Without it the response body is not SSE at all and the
/// eventsource parser yields nothing.
const ALT_SSE_QUERY: &str = "alt=sse";

/// Streaming client for the native Gemini `generateContent` API, mirroring
/// [`crate::endpoint::messages::MessagesClient`].
///
/// Auth is not handled here: the provider's `auth_header` (`x-goog-api-key`)
/// routes the caller's API key through the shared auth provider, so no
/// credential is ever constructed in this module.
pub struct GeminiClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct GeminiOptions {
    pub extra_headers: HeaderMap,
    pub compression: Compression,
    pub turn_state: Option<Arc<OnceLock<String>>>,
    /// Names of tools the request advertised as freeform (see
    /// `forall_tools::freeform_tool_names`). The SSE adapter unwraps those
    /// tool calls back into `CustomToolCall`, the only shape core can route to
    /// a freeform handler.
    pub freeform_tool_names: Vec<String>,
}

impl<T: HttpTransport> GeminiClient<T> {
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
    /// `model` is a separate parameter rather than a body field because
    /// Gemini's endpoint is model-scoped: the slug is part of the URL path.
    #[instrument(
        name = "gemini.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "gemini_http",
            http.method = "POST",
            api.path = "models:streamGenerateContent"
        )
    )]
    pub async fn stream_request(
        &self,
        model: &str,
        request: GeminiRequest,
        options: GeminiOptions,
    ) -> Result<ResponseStream, ApiError> {
        let GeminiOptions {
            extra_headers,
            compression,
            turn_state,
            freeform_tool_names,
        } = options;

        let body = EncodedJsonBody::encode(&request)
            .map_err(|e| ApiError::Stream(format!("failed to encode gemini request: {e}")))?;

        // No `session-id`/`thread-id`/`x-openai-subagent` headers here: those
        // are Forall/OpenAI backend routing affordances with no meaning to
        // Google's API. `extra_headers` still carries rollout-trace headers
        // supplied by the caller.
        self.stream_encoded(
            model,
            body,
            extra_headers,
            compression,
            turn_state,
            freeform_tool_names,
        )
        .await
    }

    fn path(model: &str) -> String {
        format!("models/{model}:streamGenerateContent")
    }

    async fn stream_encoded(
        &self,
        model: &str,
        body: EncodedJsonBody,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
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
                &Self::path(model),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    // Appended to the composed URL rather than configured as a
                    // provider `query_params` entry, so a user-supplied query
                    // string cannot displace it and `?`/`&` stays correct
                    // whether or not one is present.
                    append_alt_sse_query(&mut req.url);
                    req.compression = request_compression;
                },
            )
            .await
            .map_err(map_gemini_http_error)?;

        Ok(spawn_gemini_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
            freeform_tool_names,
        ))
    }
}

/// Appends `alt=sse` to `url`, preserving any query string already composed
/// from the provider's `query_params`. Idempotent.
fn append_alt_sse_query(url: &mut String) {
    if url
        .split_once('?')
        .is_some_and(|(_, query)| query.split('&').any(|param| param == ALT_SSE_QUERY))
    {
        return;
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);
    url.push_str(ALT_SSE_QUERY);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn path_is_model_scoped() {
        assert_eq!(
            GeminiClient::<forall_client::ReqwestTransport>::path("gemini-2.5-pro"),
            "models/gemini-2.5-pro:streamGenerateContent"
        );
    }

    #[test]
    fn alt_sse_is_appended_with_a_question_mark_when_no_query_exists() {
        let mut url =
            "https://generativelanguage.googleapis.com/v1beta/models/m:streamGenerateContent"
                .to_string();
        append_alt_sse_query(&mut url);
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/m:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn alt_sse_is_appended_with_an_ampersand_when_a_query_exists() {
        let mut url =
            "https://example.com/v1beta/models/m:streamGenerateContent?tenant=a".to_string();
        append_alt_sse_query(&mut url);
        assert_eq!(
            url,
            "https://example.com/v1beta/models/m:streamGenerateContent?tenant=a&alt=sse"
        );
    }

    #[test]
    fn alt_sse_is_not_duplicated() {
        let mut url = "https://example.com/models/m:streamGenerateContent?alt=sse".to_string();
        append_alt_sse_query(&mut url);
        assert_eq!(
            url,
            "https://example.com/models/m:streamGenerateContent?alt=sse"
        );
    }
}
