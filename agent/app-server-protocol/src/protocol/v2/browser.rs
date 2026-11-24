use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct BrowserOpenParams {
    pub thread_id: String,
    /// Absolute http(s) URL to open in the client's in-app browser pane.
    pub url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct BrowserOpenResponse {
    /// True when the client opened the URL in its in-app browser pane.
    pub opened: bool,
    /// Why the URL was not opened, when `opened` is false.
    ///
    /// This is nullable because successful opens have no error.
    pub error: Option<String>,
}
