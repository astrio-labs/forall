use std::collections::BTreeMap;

use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use forall_tools::JsonSchema;
use forall_tools::ResponsesApiTool;
use forall_tools::ToolName;
use forall_tools::ToolSpec;

const TOOL_NAME: &str = "browser_open";

#[derive(Deserialize)]
struct BrowserOpenArgs {
    url: String,
}

/// Opens a URL in the client app's built-in browser pane (not an external
/// browser). Availability depends on the connected client: the desktop app
/// answers, headless clients report the pane as unavailable.
pub struct BrowserOpenHandler;

impl ToolExecutor<ToolInvocation> for BrowserOpenHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: TOOL_NAME.to_string(),
            description: "Open a URL in the Forall app's built-in browser pane so the user can see the page alongside the conversation. Use this to show the user a website, a local dev server, or documentation. This drives the in-app browser pane only; it does not open the user's external browser and returns no page content."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "url".to_string(),
                    JsonSchema::string(Some(
                        "Absolute http or https URL to open, e.g. https://example.com or http://localhost:5173.".to_string(),
                    )),
                )]),
                Some(vec!["url".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{TOOL_NAME} handler received unsupported payload"
                )));
            };
            let BrowserOpenArgs { url } = parse_arguments(arguments)?;

            let url = url.trim().to_string();
            if !url_has_http_scheme(&url) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{TOOL_NAME} only accepts absolute http:// or https:// URLs, got: {url}"
                )));
            }

            let Some(opener) = invocation.session.services.browser_opener.clone() else {
                return Err(FunctionCallError::RespondToModel(
                    "The in-app browser pane is not available in this session.".to_string(),
                ));
            };

            let outcome = opener
                .open_url(invocation.session.thread_id, url.clone())
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "Failed to open the URL in the in-app browser: {err:#}"
                    ))
                })?;

            if !outcome.opened {
                return Err(FunctionCallError::RespondToModel(format!(
                    "The client did not open the URL in the in-app browser: {}",
                    outcome.error.as_deref().unwrap_or("no reason given")
                )));
            }

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                format!("Opened {url} in the app's browser pane."),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for BrowserOpenHandler {}

fn url_has_http_scheme(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::url_has_http_scheme;

    #[test]
    fn accepts_only_http_and_https() {
        assert!(url_has_http_scheme("https://astrio.app"));
        assert!(url_has_http_scheme("HTTP://localhost:5173/path"));
        assert!(!url_has_http_scheme("file:///etc/passwd"));
        assert!(!url_has_http_scheme("javascript:alert(1)"));
        assert!(!url_has_http_scheme("astrio.app"));
        assert!(!url_has_http_scheme("about:blank"));
    }
}
