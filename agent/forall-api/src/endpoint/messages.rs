use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::messages_common::MessagesApiRequest;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::requests::headers::build_session_headers;
use crate::requests::headers::insert_header;
use crate::requests::headers::subagent_header;
use crate::sse::messages::map_messages_http_error;
use crate::sse::spawn_messages_stream;
use crate::telemetry::SseTelemetry;
use forall_client::EncodedJsonBody;
use forall_client::HttpTransport;
use forall_client::RequestCompression;
use forall_client::RequestTelemetry;
use forall_protocol::protocol::SessionSource;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::instrument;

const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
/// Fallback API version. The effective version is expected to come from the
/// provider's `http_headers` (set alongside provider registration); this
/// constant only applies when the provider supplies none.
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// Streaming client for the Anthropic Messages API (`POST /v1/messages`),
/// mirroring [`crate::endpoint::responses::ResponsesClient`].
pub struct MessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct MessagesOptions {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_source: Option<SessionSource>,
    pub extra_headers: HeaderMap,
    pub compression: Compression,
    pub turn_state: Option<Arc<OnceLock<String>>>,
    /// Names of tools the request advertised as freeform (see
    /// `forall_tools::freeform_tool_names`). The SSE adapter unwraps those
    /// tool calls back into `CustomToolCall`, the only shape core can route to
    /// a freeform handler.
    pub freeform_tool_names: Vec<String>,
}

impl<T: HttpTransport> MessagesClient<T> {
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

    #[instrument(
        name = "messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    pub async fn stream_request(
        &self,
        request: MessagesApiRequest,
        options: MessagesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let MessagesOptions {
            session_id,
            thread_id,
            session_source,
            extra_headers,
            compression,
            turn_state,
            freeform_tool_names,
        } = options;

        let body = EncodedJsonBody::encode(&request)
            .map_err(|e| ApiError::Stream(format!("failed to encode messages request: {e}")))?;

        let mut headers = extra_headers;
        if let Some(ref thread_id) = thread_id {
            insert_header(&mut headers, "x-client-request-id", thread_id);
        }
        headers.extend(build_session_headers(session_id, thread_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        self.stream_encoded(body, headers, compression, turn_state, freeform_tool_names)
            .await
    }

    fn path() -> &'static str {
        "messages"
    }

    async fn stream_encoded(
        &self,
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
                Self::path(),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    if !req.headers.contains_key(ANTHROPIC_VERSION_HEADER) {
                        req.headers.insert(
                            ANTHROPIC_VERSION_HEADER,
                            HeaderValue::from_static(DEFAULT_ANTHROPIC_VERSION),
                        );
                    }
                    req.compression = request_compression;
                },
            )
            .await
            .map_err(map_messages_http_error)?;

        Ok(spawn_messages_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
            freeform_tool_names,
        ))
    }
}
