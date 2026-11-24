use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use forall_protocol::ThreadId;

/// Outcome of asking the client to open a URL in its in-app browser pane.
#[derive(Debug, Clone)]
pub struct BrowserOpenOutcome {
    pub opened: bool,
    /// Why the URL was not opened, when `opened` is false.
    pub error: Option<String>,
}

pub type BrowserOpenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BrowserOpenOutcome>> + Send + 'a>>;

/// Host integration boundary for the client-owned in-app browser pane.
///
/// There is no engine-side fallback: sessions without a client that owns a
/// browser pane simply have no opener, and the tool reports that to the model.
pub trait BrowserOpener: Send + Sync {
    fn open_url(&self, thread_id: ThreadId, url: String) -> BrowserOpenFuture<'_>;
}
