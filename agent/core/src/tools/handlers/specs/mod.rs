//! Native Forall spec workflow tools (namespace `specs`).

mod runner;
mod spec;

use runner::run_specs_command;
use runner::run_specs_command_with_text;
use spec::SPECS_NAMESPACE;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use forall_tools::ToolName;
use forall_tools::ToolSpec;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ProposeArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChangeArgs {
    change: String,
}

#[derive(Debug, Deserialize)]
struct InstructionsArgs {
    change: String,
    artifact: String,
}

#[derive(Debug, Deserialize)]
struct CheckArgs {
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    strict: Option<bool>,
    #[serde(default)]
    pbt_seed: Option<u64>,
    #[serde(default)]
    pbt_examples: Option<u32>,
}

pub struct SpecsListHandler;

impl ToolExecutor<ToolInvocation> for SpecsListHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "list")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_list_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            parse_specs_args(&invocation)?;
            run_specs_command(&invocation, vec!["list".into(), "--json".into()]).await
        })
    }
}

impl CoreToolRuntime for SpecsListHandler {}

pub struct SpecsProposeHandler;

impl ToolExecutor<ToolInvocation> for SpecsProposeHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "propose")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_propose_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: ProposeArgs = parse_arguments(&args)?;
            run_specs_command(&invocation, vec!["propose".into(), parsed.name]).await
        })
    }
}

impl CoreToolRuntime for SpecsProposeHandler {}

pub struct SpecsStatusHandler;

impl ToolExecutor<ToolInvocation> for SpecsStatusHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "status")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_status_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: ChangeArgs = parse_arguments(&args)?;
            run_specs_command(
                &invocation,
                vec![
                    "status".into(),
                    "--change".into(),
                    parsed.change,
                    "--json".into(),
                ],
            )
            .await
        })
    }
}

impl CoreToolRuntime for SpecsStatusHandler {}

pub struct SpecsInstructionsHandler;

impl ToolExecutor<ToolInvocation> for SpecsInstructionsHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "instructions")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_instructions_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: InstructionsArgs = parse_arguments(&args)?;
            run_specs_command(
                &invocation,
                vec![
                    "instructions".into(),
                    parsed.artifact,
                    "--change".into(),
                    parsed.change,
                    "--plain".into(),
                ],
            )
            .await
        })
    }
}

impl CoreToolRuntime for SpecsInstructionsHandler {}

pub struct SpecsCheckHandler;

impl ToolExecutor<ToolInvocation> for SpecsCheckHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "check")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_check_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: CheckArgs = parse_arguments(&args)?;
            let mut cli_args = vec!["check".into(), "--json".into()];
            if let Some(change) = parsed.change {
                cli_args.push("--change".into());
                cli_args.push(change);
            }
            if parsed.strict.unwrap_or(false) {
                cli_args.push("--strict".into());
            }
            if let Some(seed) = parsed.pbt_seed {
                cli_args.push("--pbt-seed".into());
                cli_args.push(seed.to_string());
            }
            if let Some(examples) = parsed.pbt_examples {
                cli_args.push("--pbt-examples".into());
                cli_args.push(examples.to_string());
            }
            let started = std::time::Instant::now();
            let (output, raw_text) = run_specs_command_with_text(&invocation, cli_args).await?;
            if let Ok(report) =
                serde_json::from_str::<forall_workflow::mapping::schema::VerifyReport>(&raw_text)
            {
                crate::verified_engineering_analytics::track_forall_check_completed(
                    &invocation.session.services.analytics_events_client,
                    &invocation.turn,
                    &invocation.session.thread_id().to_string(),
                    &report,
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "specs.check",
                );
            }
            Ok(output)
        })
    }
}

impl CoreToolRuntime for SpecsCheckHandler {}

pub struct SpecsSyncHandler;

impl ToolExecutor<ToolInvocation> for SpecsSyncHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "sync")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_sync_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: ChangeArgs = parse_arguments(&args)?;
            run_specs_command(
                &invocation,
                vec!["sync".into(), "--change".into(), parsed.change],
            )
            .await
        })
    }
}

impl CoreToolRuntime for SpecsSyncHandler {}

pub struct SpecsArchiveHandler;

impl ToolExecutor<ToolInvocation> for SpecsArchiveHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(SPECS_NAMESPACE, "archive")
    }

    fn spec(&self) -> ToolSpec {
        spec::create_archive_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = parse_specs_args(&invocation)?;
            let parsed: ChangeArgs = parse_arguments(&args)?;
            run_specs_command(&invocation, vec!["archive".into(), parsed.change]).await
        })
    }
}

impl CoreToolRuntime for SpecsArchiveHandler {}

fn parse_specs_args(invocation: &ToolInvocation) -> Result<String, FunctionCallError> {
    match &invocation.payload {
        ToolPayload::Function { arguments } => Ok(arguments.clone()),
        _ => Err(FunctionCallError::RespondToModel(
            "specs tool received unsupported payload".to_string(),
        )),
    }
}
