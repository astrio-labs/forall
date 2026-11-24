use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::resolve_tool_environment;
use forall_agent_bridge::run_forall_cli;
use forall_protocol::items::SpecsToolCallItem;
use forall_protocol::items::SpecsToolCallStatus;
use forall_protocol::items::TurnItem;
use std::sync::Arc;
use std::time::Instant;

pub async fn run_specs_command(
    invocation: &ToolInvocation,
    cli_args: Vec<String>,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let (output, _) = run_specs_command_with_text(invocation, cli_args).await?;
    Ok(output)
}

pub async fn run_specs_command_with_text(
    invocation: &ToolInvocation,
    cli_args: Vec<String>,
) -> Result<(Box<dyn crate::tools::context::ToolOutput>, String), FunctionCallError> {
    // specs.* tools run natively in the engine and leave no raw response item
    // on the wire, so this TurnItem lifecycle is their only transcript record;
    // its completion is persisted for thread/resume replay.
    let item = SpecsToolCallItem {
        id: invocation.call_id.clone(),
        tool: invocation.tool_name.name.clone(),
        arguments: parse_specs_arguments(&invocation.payload),
        status: SpecsToolCallStatus::InProgress,
        output: None,
        success: None,
        duration: None,
    };
    invocation
        .session
        .emit_turn_item_started(
            invocation.turn.as_ref(),
            &TurnItem::SpecsToolCall(item.clone()),
        )
        .await;

    // A turn abort drops this future wherever it is suspended; the guard
    // guarantees a terminal completion is still emitted (and persisted) so
    // clients never keep an in-progress specs cell forever.
    let mut lifecycle = SpecsToolCallLifecycle {
        session: invocation.session.clone(),
        turn: invocation.turn.clone(),
        item: Some(item),
        started: Instant::now(),
    };

    let result = execute_specs_command(invocation, cli_args).await;
    let (status, output, success) = match &result {
        Ok((output, success)) => (
            if *success {
                SpecsToolCallStatus::Completed
            } else {
                SpecsToolCallStatus::Failed
            },
            output.clone(),
            *success,
        ),
        Err(err) => (SpecsToolCallStatus::Failed, err.to_string(), false),
    };
    lifecycle.complete(status, output, success).await;

    let (output, success) = result?;
    Ok((
        boxed_tool_output(FunctionToolOutput::from_text(output.clone(), Some(success))),
        output,
    ))
}

struct SpecsToolCallLifecycle {
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    /// `Some` while the terminal completion still needs to be emitted.
    item: Option<SpecsToolCallItem>,
    started: Instant,
}

impl SpecsToolCallLifecycle {
    async fn complete(&mut self, status: SpecsToolCallStatus, output: String, success: bool) {
        let Some(item) = self.item.take() else {
            return;
        };
        self.session
            .emit_turn_item_completed(
                self.turn.as_ref(),
                TurnItem::SpecsToolCall(SpecsToolCallItem {
                    status,
                    output: Some(output),
                    success: Some(success),
                    duration: Some(self.started.elapsed()),
                    ..item
                }),
            )
            .await;
    }
}

impl Drop for SpecsToolCallLifecycle {
    fn drop(&mut self) {
        let Some(item) = self.item.take() else {
            return;
        };
        // Dropped before completing: the turn was interrupted mid-execution.
        // Emission is async, so hand it to the runtime; if none is available
        // (process teardown) there is no one left to notify anyway.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let session = self.session.clone();
        let turn = self.turn.clone();
        let duration = self.started.elapsed();
        handle.spawn(async move {
            session
                .emit_turn_item_completed(
                    turn.as_ref(),
                    TurnItem::SpecsToolCall(SpecsToolCallItem {
                        status: SpecsToolCallStatus::Failed,
                        output: Some(
                            "turn interrupted before the specs tool finished".to_string(),
                        ),
                        success: Some(false),
                        duration: Some(duration),
                        ..item
                    }),
                )
                .await;
        });
    }
}

async fn execute_specs_command(
    invocation: &ToolInvocation,
    cli_args: Vec<String>,
) -> Result<(String, bool), FunctionCallError> {
    let turn_environment = resolve_tool_environment(&invocation.turn, None)?.ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "specs tools are unavailable without a turn environment".to_string(),
        )
    })?;
    let cwd = turn_environment.cwd().to_abs_path().map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve project cwd: {err}"))
    })?;
    let refs: Vec<&str> = cli_args.iter().map(String::as_str).collect();
    let (code, output) = run_forall_cli(&cwd, &refs)
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format!("forall CLI failed: {err}")))?;

    Ok((output, code == 0))
}

fn parse_specs_arguments(payload: &ToolPayload) -> serde_json::Value {
    match payload {
        ToolPayload::Function { arguments } => {
            serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null)
        }
        _ => serde_json::Value::Null,
    }
}
