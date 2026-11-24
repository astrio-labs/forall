use std::sync::Arc;
use std::sync::Weak;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use forall_app_server_protocol::BrowserOpenParams;
use forall_app_server_protocol::BrowserOpenResponse;
use forall_app_server_protocol::ServerRequestPayload;
use forall_core::BrowserOpenFuture;
use forall_core::BrowserOpenOutcome;
use forall_core::BrowserOpener;
use forall_protocol::ThreadId;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::timeout_at;

use crate::outgoing_message::OutgoingMessageSender;
use crate::thread_state::ThreadStateManager;

/// The client answers as soon as it has dispatched the URL to its browser
/// pane (it does not wait for the page to load), so a short budget is enough.
const BROWSER_OPEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn app_server_browser_opener(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
) -> Arc<dyn BrowserOpener> {
    Arc::new(AppServerBrowserOpener {
        outgoing: Arc::downgrade(&outgoing),
        thread_state_manager,
    })
}

struct AppServerBrowserOpener {
    outgoing: Weak<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
}

impl BrowserOpener for AppServerBrowserOpener {
    fn open_url(&self, thread_id: ThreadId, url: String) -> BrowserOpenFuture<'_> {
        let outgoing = self.outgoing.clone();
        let thread_state_manager = self.thread_state_manager.clone();
        Box::pin(async move {
            let outgoing = outgoing
                .upgrade()
                .context("app-server browser opener is unavailable")?;
            request_browser_open(outgoing, thread_state_manager, thread_id, url).await
        })
    }
}

async fn request_browser_open(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
    thread_id: ThreadId,
    url: String,
) -> Result<BrowserOpenOutcome> {
    let deadline = Instant::now() + BROWSER_OPEN_REQUEST_TIMEOUT;
    timeout_at(
        deadline,
        thread_state_manager.wait_for_thread_subscriber(thread_id),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "timed out waiting for a client to subscribe to the thread after {}s",
            BROWSER_OPEN_REQUEST_TIMEOUT.as_secs()
        )
    })?;
    let connection_ids = thread_state_manager
        .subscribed_connection_ids(thread_id)
        .await;
    // The pane belongs to whichever client renders the thread; with several
    // subscribed there is no single pane to target, so refuse rather than
    // guess (mirrors the external current-time clock rule).
    let connection_id = match connection_ids.as_slice() {
        [connection_id] => *connection_id,
        _ => bail!(
            "expected exactly one client subscribed to the thread, found {}",
            connection_ids.len()
        ),
    };
    let connection_ids = [connection_id];
    let (request_id, rx) = outgoing
        .send_request_to_connections(
            Some(&connection_ids),
            ServerRequestPayload::BrowserOpen(BrowserOpenParams {
                thread_id: thread_id.to_string(),
                url,
            }),
            /*thread_id*/ None,
        )
        .await;

    let result = match timeout_at(deadline, rx).await {
        Ok(Ok(Ok(result))) => result,
        Ok(Ok(Err(err))) => {
            // A structured client refusal (e.g. the TUI's "unsupported") is an
            // outcome for the model, not an engine error.
            return Ok(BrowserOpenOutcome {
                opened: false,
                error: Some(err.message),
            });
        }
        Ok(Err(err)) => bail!("browser-open request was canceled: {err}"),
        Err(_) => {
            let _canceled = outgoing.cancel_request(&request_id).await;
            bail!(
                "browser-open request timed out after {}s",
                BROWSER_OPEN_REQUEST_TIMEOUT.as_secs()
            );
        }
    };
    let response: BrowserOpenResponse =
        serde_json::from_value(result).context("invalid browser-open response")?;

    Ok(BrowserOpenOutcome {
        opened: response.opened,
        error: response.error,
    })
}
