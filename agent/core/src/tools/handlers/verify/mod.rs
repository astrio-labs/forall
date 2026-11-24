//! Native hosted Forall verification tools (namespace `verify`).

mod spec;

use std::path::PathBuf;
use std::sync::Arc;

use forall_hosted_verify::ExplainVerificationRequest;
use forall_hosted_verify::ExplanationAudience;
use forall_hosted_verify::HostedVerificationClient;
use forall_hosted_verify::SnapshotPacker;
use forall_hosted_verify::StaticBearerTokenProvider;
use forall_hosted_verify::SubmitVerificationRequest;
use forall_hosted_verify::VerificationPhase;
use forall_hosted_verify::VerificationScope;
use forall_hosted_verify::VerificationSource;
use forall_login::VerificationApiKeyStore;
use forall_tools::JsonToolOutput;
use forall_tools::ToolName;
use forall_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

use spec::VERIFY_NAMESPACE;

const ENDPOINT_ENV: &str = "FORALL_VERIFICATION_MCP_URL";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SubmitSourceInput {
    Workspace {
        #[serde(default)]
        paths: Vec<String>,
    },
    Github {
        repository: String,
        #[serde(rename = "ref", default = "default_git_ref")]
        reference: String,
        #[serde(default)]
        subdirectory: Option<String>,
    },
}

fn default_git_ref() -> String {
    "main".to_string()
}

#[derive(Debug, Deserialize)]
struct SubmitInput {
    #[serde(default)]
    source: Option<SubmitSourceInput>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    phases: Vec<VerificationPhase>,
    #[serde(default)]
    pbt_seed: Option<u64>,
    #[serde(default)]
    pbt_examples: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct JobInput {
    job_id: String,
}

#[derive(Debug, Deserialize)]
struct ExplainInput {
    job_id: String,
    #[serde(default)]
    issue_indexes: Vec<usize>,
    #[serde(default)]
    audience: Option<ExplanationAudience>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyOperation {
    Submit,
    Status,
    Cancel,
    Explain,
}

impl VerifyOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Submit => "submit",
            Self::Status => "status",
            Self::Cancel => "cancel",
            Self::Explain => "explain",
        }
    }

    fn spec(self) -> ToolSpec {
        match self {
            Self::Submit => spec::create_submit_tool(),
            Self::Status => spec::create_status_tool(),
            Self::Cancel => spec::create_cancel_tool(),
            Self::Explain => spec::create_explain_tool(),
        }
    }
}

pub struct VerifyHandler {
    operation: VerifyOperation,
}

impl VerifyHandler {
    const fn new(operation: VerifyOperation) -> Self {
        Self { operation }
    }

    pub const fn submit() -> Self {
        Self::new(VerifyOperation::Submit)
    }

    pub const fn status() -> Self {
        Self::new(VerifyOperation::Status)
    }

    pub const fn cancel() -> Self {
        Self::new(VerifyOperation::Cancel)
    }

    pub const fn explain() -> Self {
        Self::new(VerifyOperation::Explain)
    }

    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let arguments = match &invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "verify tool received unsupported payload".to_string(),
                ));
            }
        };
        let result = match self.operation {
            VerifyOperation::Submit => {
                let input: SubmitInput = parse_arguments(arguments)?;
                self.submit_job(&invocation, input).await
            }
            VerifyOperation::Status => {
                let input: JobInput = parse_arguments(arguments)?;
                self.with_client(&invocation, |client| async move {
                    serialize_result(client.status(input.job_id).await)
                })
                .await
            }
            VerifyOperation::Cancel => {
                let input: JobInput = parse_arguments(arguments)?;
                self.with_client(&invocation, |client| async move {
                    serialize_result(client.cancel(input.job_id).await)
                })
                .await
            }
            VerifyOperation::Explain => {
                let input: ExplainInput = parse_arguments(arguments)?;
                self.with_client(&invocation, |client| async move {
                    serialize_result(
                        client
                            .explain(&ExplainVerificationRequest {
                                job_id: input.job_id,
                                issue_indexes: input.issue_indexes,
                                audience: input.audience.unwrap_or(ExplanationAudience::Concise),
                            })
                            .await,
                    )
                })
                .await
            }
        };
        Ok(output(result))
    }

    async fn submit_job(
        &self,
        invocation: &ToolInvocation,
        input: SubmitInput,
    ) -> Result<serde_json::Value, VerifyToolError> {
        let source = match input.source {
            Some(SubmitSourceInput::Github {
                repository,
                reference,
                subdirectory,
            }) => VerificationSource::github(repository, reference, subdirectory),
            source => {
                let root = project_root(invocation)?;
                let paths = match source {
                    Some(SubmitSourceInput::Workspace { paths }) => paths,
                    None => Vec::new(),
                    Some(SubmitSourceInput::Github { .. }) => unreachable!(),
                };
                tokio::task::spawn_blocking(move || {
                    let packer = SnapshotPacker::default();
                    if paths.is_empty() {
                        packer.pack_verification_workspace(&root)
                    } else {
                        packer.pack_paths(&root, paths)
                    }
                })
                .await
                .map_err(|error| VerifyToolError::snapshot(error.to_string()))?
                .map_err(|error| VerifyToolError::snapshot(error.to_string()))?
            }
        };
        let request = SubmitVerificationRequest {
            source,
            scope: input.change.map_or(VerificationScope::Project, |name| {
                VerificationScope::Change { name }
            }),
            strict: input.strict,
            phases: input.phases,
            pbt_seed: input.pbt_seed,
            pbt_examples: input.pbt_examples,
            // Whole-project verification; requirement scoping is a
            // hosted-API capability the CLI does not surface yet.
            requirement_ids: None,
        };
        self.with_client(invocation, |client| async move {
            serialize_result(client.submit(&request).await)
        })
        .await
    }

    async fn with_client<F, Fut>(
        &self,
        invocation: &ToolInvocation,
        operation: F,
    ) -> Result<serde_json::Value, VerifyToolError>
    where
        F: FnOnce(HostedVerificationClient) -> Fut,
        Fut: std::future::Future<Output = Result<serde_json::Value, VerifyToolError>>,
    {
        let forall_home = invocation.turn.config.forall_home.to_path_buf();
        let api_key =
            tokio::task::spawn_blocking(move || VerificationApiKeyStore::new(&forall_home).get())
                .await
                .map_err(|error| VerifyToolError::auth(error.to_string()))?
                .map_err(|error| VerifyToolError::auth(error.to_string()))?
                .ok_or_else(VerifyToolError::missing_auth)?;
        let provider = StaticBearerTokenProvider::new(api_key)
            .map_err(|error| VerifyToolError::auth(error.to_string()))?;
        let endpoint = std::env::var(ENDPOINT_ENV)
            .unwrap_or_else(|_| forall_hosted_verify::DEFAULT_HOSTED_MCP_ENDPOINT.to_string());
        let client = HostedVerificationClient::with_endpoint(endpoint, Arc::new(provider));
        tokio::select! {
            _ = invocation.cancellation_token.cancelled() => {
                Err(VerifyToolError::cancelled())
            }
            result = operation(client) => result,
        }
    }
}

impl ToolExecutor<ToolInvocation> for VerifyHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(VERIFY_NAMESPACE, self.operation.name())
    }

    fn spec(&self) -> ToolSpec {
        self.operation.spec()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for VerifyHandler {}

fn project_root(invocation: &ToolInvocation) -> Result<PathBuf, VerifyToolError> {
    let cwd = resolve_tool_environment(&invocation.turn, None)
        .map_err(|error| VerifyToolError::snapshot(error.to_string()))?
        .ok_or_else(|| VerifyToolError::snapshot("verify tools require a turn environment"))?
        .cwd()
        .to_abs_path()
        .map_err(|error| VerifyToolError::snapshot(error.to_string()))?;
    Ok(forall_workflow::paths::find_project_root(&cwd).unwrap_or_else(|| cwd.to_path_buf()))
}

fn serialize_result<T: Serialize, E: std::fmt::Display>(
    result: Result<T, E>,
) -> Result<serde_json::Value, VerifyToolError> {
    result
        .map_err(|error| VerifyToolError::hosted(error.to_string()))
        .and_then(|value| {
            serde_json::to_value(value).map_err(|error| VerifyToolError::hosted(error.to_string()))
        })
}

fn output(
    result: Result<serde_json::Value, VerifyToolError>,
) -> Box<dyn crate::tools::context::ToolOutput> {
    match result {
        Ok(result) => Box::new(JsonToolOutput::new(serde_json::json!({
            "ok": true,
            "result": result,
        }))),
        Err(error) => Box::new(JsonToolOutput::with_success(
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": error.code,
                    "message": error.message,
                },
            }),
            Some(false),
        )),
    }
}

struct VerifyToolError {
    code: &'static str,
    message: String,
}

impl VerifyToolError {
    fn auth(message: impl Into<String>) -> Self {
        Self {
            code: "authentication_error",
            message: message.into(),
        }
    }

    fn missing_auth() -> Self {
        Self {
            code: "authentication_required",
            message: "Hosted verification is not configured. Pipe your Forall API key to `forall verification login`, or set FORALL_API_KEY.".to_string(),
        }
    }

    fn snapshot(message: impl Into<String>) -> Self {
        Self {
            code: "snapshot_error",
            message: message.into(),
        }
    }

    fn hosted(message: impl Into<String>) -> Self {
        Self {
            code: "hosted_verification_error",
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self {
            code: "cancelled",
            message: "Hosted verification request was cancelled.".to_string(),
        }
    }
}
