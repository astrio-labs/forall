//! Loopback flow that connects the user's GitHub account for repository
//! access.
//!
//! Mirrors [`crate::oauth_flow`]: bind an ephemeral loopback server, ask the
//! Forall auth service for a GitHub authorize URL
//! (`POST /connections/github/start`, authenticated with the account access
//! token so the identity is linked server-side), wait for the browser
//! redirect, and redeem the single-use PKCE-bound code at
//! `POST /connections/github/token`. The GitHub token that comes back is
//! persisted in auth.json only — the server never stores it durably.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use url::Url;

use crate::auth::AuthBaseUrls;
use crate::auth::AuthKeyringBackendKind;
use crate::auth::save_github_connection;
use crate::oauth_flow::CancelHandle;
use crate::oauth_flow::error_page_response;
use crate::oauth_flow::generate_pkce;
use crate::oauth_flow::html_response;
use crate::oauth_flow::lock_cancelled;
use crate::oauth_flow::plain_response;
use crate::oauth_flow::respond;
use forall_config::types::AuthCredentialsStoreMode;

/// Default overall timeout for the connect flow.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(600);

const CALLBACK_PATH: &str = "/callback";
const GITHUB_SUCCESS_HTML: &str = include_str!("assets/github_success.html");

/// A GitHub account connected for repository access, persisted in auth.json.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubConnection {
    /// GitHub OAuth access token; used as the password for `git` over HTTPS
    /// and for GitHub API calls.
    pub access_token: String,
    /// GitHub username, for display.
    pub login: String,
    /// Scopes granted to the token.
    #[serde(default)]
    pub scopes: Vec<String>,
    pub connected_at: DateTime<Utc>,
}

/// Manual `Debug` so a debug-formatted auth payload (which tracing may
/// capture) never carries the token.
impl std::fmt::Debug for GithubConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubConnection")
            .field("access_token", &"<redacted>")
            .field("login", &self.login)
            .field("scopes", &self.scopes)
            .field("connected_at", &self.connected_at)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GithubConnectError {
    #[error("sign in to Forall before connecting GitHub")]
    NotSignedIn,
    #[error("failed to start the GitHub connect flow: {0}")]
    Start(String),
    #[error("timed out waiting for GitHub authorization to complete")]
    TimedOut,
    #[error("GitHub connect was cancelled")]
    Cancelled,
    #[error("state parameter returned by the auth service did not match")]
    StateMismatch,
    #[error("callback request did not include an authorization code")]
    MissingCode,
    #[error("failed to redeem the connect code: {0}")]
    Exchange(String),
    #[error("failed to persist the GitHub connection: {0}")]
    Persist(String),
    #[error("loopback server error: {0}")]
    Server(String),
}

/// Options for [`GithubConnectFlow::start`].
#[derive(Debug, Clone)]
pub struct GithubConnectOptions {
    pub base_urls: AuthBaseUrls,
    /// Forall account access token (JWT); authenticates the start request.
    pub account_access_token: String,
    pub forall_home: PathBuf,
    pub store_mode: AuthCredentialsStoreMode,
    pub keyring_kind: AuthKeyringBackendKind,
    /// When true, attempt to open the authorization URL in the default
    /// browser; callers should surface [`StartedGithubConnect::auth_url`]
    /// as a fallback either way.
    pub open_browser: bool,
    pub timeout: Duration,
}

/// A started connect flow; `completion` resolves exactly once.
pub struct StartedGithubConnect {
    /// GitHub authorization URL the user's browser must visit.
    pub auth_url: String,
    pub completion: tokio::sync::oneshot::Receiver<Result<GithubConnection, GithubConnectError>>,
    pub cancel: CancelHandle,
}

pub struct GithubConnectFlow;

impl GithubConnectFlow {
    /// Binds the loopback server, asks the auth service for the authorize
    /// URL, and spawns the background worker that waits for the browser.
    pub async fn start(
        opts: GithubConnectOptions,
    ) -> Result<StartedGithubConnect, GithubConnectError> {
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|err| {
            GithubConnectError::Server(format!("failed to bind loopback server: {err}"))
        })?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| {
                GithubConnectError::Server("loopback server has no IP address".to_string())
            })?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

        let pkce = generate_pkce();
        let state = crate::oauth_flow::generate_state();

        // The start request is authenticated and must happen before the
        // browser opens: the authorize URL comes from the auth service.
        #[derive(Deserialize)]
        struct StartResponse {
            authorize_url: String,
        }
        let response = crate::auth::default_client::credential_request(
            reqwest::Method::POST,
            &format!("{}/connections/github/start", opts.base_urls.base_url()),
        )
        .map_err(|err| GithubConnectError::Start(err.to_string()))?
        .bearer_auth(&opts.account_access_token)
        .form(&[
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|err| GithubConnectError::Start(err.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GithubConnectError::NotSignedIn);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(GithubConnectError::Start(format!(
                "auth service returned HTTP {status}: {body}"
            )));
        }
        let StartResponse { authorize_url } = response
            .json()
            .await
            .map_err(|err| GithubConnectError::Start(format!("invalid start response: {err}")))?;

        let server = Arc::new(server);
        let cancelled = Arc::new(Mutex::new(false));
        let cancel = CancelHandle::new(Arc::clone(&cancelled), Arc::clone(&server));

        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let worker = ConnectWorker {
            server,
            cancelled,
            state,
            pkce_verifier: pkce.verifier,
            redirect_uri,
            token_url: format!("{}/connections/github/token", opts.base_urls.base_url()),
            forall_home: opts.forall_home,
            store_mode: opts.store_mode,
            keyring_kind: opts.keyring_kind,
            timeout: opts.timeout,
        };
        std::thread::spawn(move || {
            let result = worker.run();
            // The receiver may have been dropped; nothing to do in that case.
            let _ = completion_tx.send(result);
        });

        if opts.open_browser
            && let Err(err) = webbrowser::open(&authorize_url)
        {
            tracing::warn!("failed to open browser for GitHub connect: {err}");
        }

        Ok(StartedGithubConnect {
            auth_url: authorize_url,
            completion: completion_rx,
            cancel,
        })
    }
}

struct ConnectWorker {
    server: Arc<tiny_http::Server>,
    cancelled: Arc<Mutex<bool>>,
    state: String,
    pkce_verifier: String,
    redirect_uri: String,
    token_url: String,
    forall_home: PathBuf,
    store_mode: AuthCredentialsStoreMode,
    keyring_kind: AuthKeyringBackendKind,
    timeout: Duration,
}

impl ConnectWorker {
    fn is_cancelled(&self) -> bool {
        *lock_cancelled(&self.cancelled)
    }

    fn run(&self) -> Result<GithubConnection, GithubConnectError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.is_cancelled() {
                return Err(GithubConnectError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(GithubConnectError::TimedOut);
            }
            let request = match self.server.recv_timeout(deadline - now) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    // `recv_timeout` returns `None` both on timeout and when
                    // `CancelHandle::cancel` unblocks the server.
                    if self.is_cancelled() {
                        return Err(GithubConnectError::Cancelled);
                    }
                    return Err(GithubConnectError::TimedOut);
                }
                Err(err) => {
                    if self.is_cancelled() {
                        return Err(GithubConnectError::Cancelled);
                    }
                    return Err(GithubConnectError::Server(err.to_string()));
                }
            };

            if self.is_cancelled() {
                respond(
                    request,
                    error_page_response(
                        "GitHub connect cancelled",
                        "This connection was cancelled from Forall. You can close this tab.",
                        200,
                    ),
                );
                return Err(GithubConnectError::Cancelled);
            }

            let callback_url =
                match Url::parse("http://127.0.0.1").and_then(|base| base.join(request.url())) {
                    Ok(url) => url,
                    Err(_) => {
                        respond(request, plain_response("Bad request", 400));
                        continue;
                    }
                };
            if callback_url.path() != CALLBACK_PATH {
                respond(request, plain_response("Not found", 404));
                continue;
            }

            let mut code = None;
            let mut state = None;
            for (key, value) in callback_url.query_pairs() {
                match key.as_ref() {
                    "code" => code = Some(value.into_owned()),
                    "state" => state = Some(value.into_owned()),
                    _ => {}
                }
            }

            if state.as_deref() != Some(self.state.as_str()) {
                let err = GithubConnectError::StateMismatch;
                respond(
                    request,
                    error_page_response(
                        "GitHub connection failed",
                        &format!("{err}. You can close this tab and try again from Forall."),
                        400,
                    ),
                );
                return Err(err);
            }
            let Some(code) = code else {
                let err = GithubConnectError::MissingCode;
                respond(
                    request,
                    error_page_response(
                        "GitHub connection failed",
                        &format!("{err}. You can close this tab and try again from Forall."),
                        400,
                    ),
                );
                return Err(err);
            };

            // Redeem first (network I/O, no lock held), then decide
            // atomically below whether the connection may be persisted.
            let connection = match self.exchange_code(&code) {
                Ok(connection) => connection,
                Err(err) => {
                    respond(
                        request,
                        error_page_response(
                            "GitHub connect failed",
                            &format!("{err}. You can close this tab and try again from Forall."),
                            500,
                        ),
                    );
                    return Err(err);
                }
            };

            // Cancel-vs-persist critical section; see
            // `crate::oauth_flow::CancelHandle::cancel` for the invariant.
            let cancelled_guard = lock_cancelled(&self.cancelled);
            if *cancelled_guard {
                drop(cancelled_guard);
                respond(
                    request,
                    error_page_response(
                        "GitHub connect cancelled",
                        "This connection was cancelled from Forall. You can close this tab.",
                        200,
                    ),
                );
                return Err(GithubConnectError::Cancelled);
            }
            let save_result = save_github_connection(
                &self.forall_home,
                &connection,
                self.store_mode,
                self.keyring_kind,
            )
            .map_err(|err| GithubConnectError::Persist(err.to_string()));
            drop(cancelled_guard);

            return match save_result {
                Ok(()) => {
                    respond(request, html_response(GITHUB_SUCCESS_HTML));
                    Ok(connection)
                }
                Err(err) => {
                    respond(
                        request,
                        error_page_response(
                            "GitHub connect failed",
                            &format!("{err}. You can close this tab and try again from Forall."),
                            500,
                        ),
                    );
                    Err(err)
                }
            };
        }
    }

    fn exchange_code(&self, code: &str) -> Result<GithubConnection, GithubConnectError> {
        #[derive(Deserialize)]
        struct ConnectTokenResponse {
            access_token: String,
            github_login: String,
            #[serde(default)]
            scopes: Vec<String>,
        }

        let response = crate::auth::default_client::blocking_credential_post(&self.token_url)
            .map_err(|err| GithubConnectError::Exchange(err.to_string()))?
            .form(&[
                ("code", code),
                ("code_verifier", self.pkce_verifier.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .send()
            .map_err(|err| GithubConnectError::Exchange(err.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(GithubConnectError::Exchange(format!(
                "connect token endpoint returned HTTP {status}: {body}"
            )));
        }
        let token: ConnectTokenResponse = response.json().map_err(|err| {
            GithubConnectError::Exchange(format!("invalid token response: {err}"))
        })?;
        Ok(GithubConnection {
            access_token: token.access_token,
            login: token.github_login,
            scopes: token.scopes,
            connected_at: Utc::now(),
        })
    }
}

/// Speak the `git credential` helper protocol for the stored connection.
///
/// This exists so the connection token never has to travel through git's
/// environment (which hooks, filters and other git children inherit): the
/// engine points `credential.https://github.com.helper` at
/// `forall git-credential-github`, and git invokes it with the operation as
/// argv and `key=value` lines on stdin. Only `get` for `https`/`github.com`
/// answers; every other operation or host writes nothing, which git treats
/// as "this helper has no opinion".
pub fn respond_to_git_credential_request(
    operation: &str,
    input: &str,
    connection: Option<&GithubConnection>,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    if operation != "get" {
        return Ok(());
    }
    let mut protocol = None;
    let mut host = None;
    for line in input.lines() {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "protocol" => protocol = Some(value),
                "host" => host = Some(value),
                _ => {}
            }
        }
    }
    if protocol != Some("https") || host != Some("github.com") {
        return Ok(());
    }
    let Some(connection) = connection else {
        return Ok(());
    };
    writeln!(output, "username=x-access-token")?;
    writeln!(output, "password={}", connection.access_token)
}

/// Disconnect GitHub: best-effort upstream revocation through the auth
/// service, then remove the stored connection. Returns whether a connection
/// was present.
pub async fn disconnect_github(
    forall_home: &std::path::Path,
    base_urls: &AuthBaseUrls,
    store_mode: AuthCredentialsStoreMode,
    keyring_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    let Some(connection) =
        crate::auth::load_github_connection(forall_home, store_mode, keyring_kind)?
    else {
        return Ok(false);
    };

    let revoke = async {
        crate::auth::default_client::credential_request(
            reqwest::Method::POST,
            &format!("{}/connections/github/revoke", base_urls.base_url()),
        )?
        .form(&[("token", connection.access_token.as_str())])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(std::io::Error::other)
    };
    match revoke.await {
        Ok(response) if !response.status().is_success() => {
            tracing::warn!(
                "github revoke endpoint returned HTTP {} during disconnect",
                response.status()
            );
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!("failed to revoke GitHub token during disconnect: {err}");
        }
    }

    crate::auth::clear_github_connection(forall_home, store_mode, keyring_kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::load_github_connection;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    /// Serves the start request (echoing the agent's redirect_uri and state
    /// into the authorize URL, the way GitHub's real URL carries our signed
    /// state) and then exactly one token-exchange request.
    fn spawn_mock_auth_service() -> (String, std::thread::JoinHandle<HashMap<String, String>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            // 1: /connections/github/start
            let mut request = server.recv().unwrap();
            assert_eq!(request.url(), "/connections/github/start");
            let authorization = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| header.value.as_str().to_string());
            assert_eq!(authorization.as_deref(), Some("Bearer account-jwt"));
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let start_fields: HashMap<String, String> =
                url::form_urlencoded::parse(body.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
            assert_eq!(start_fields.get("code_challenge_method").unwrap(), "S256");
            let mut authorize_url =
                Url::parse("https://github.com/login/oauth/authorize?client_id=x").unwrap();
            authorize_url
                .query_pairs_mut()
                .append_pair("redirect_uri", start_fields.get("redirect_uri").unwrap())
                .append_pair("state", start_fields.get("state").unwrap());
            let response_body =
                serde_json::json!({ "authorize_url": authorize_url.as_str() }).to_string();
            let response = tiny_http::Response::from_string(response_body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            );
            request.respond(response).unwrap();

            // 2: /connections/github/token
            let mut request = server.recv().unwrap();
            assert_eq!(request.url(), "/connections/github/token");
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let fields: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let response_body = serde_json::json!({
                "access_token": "gho_test_token",
                "token_type": "bearer",
                "github_login": "octocat",
                "scopes": ["repo", "read:org", "workflow"],
            })
            .to_string();
            let response = tiny_http::Response::from_string(response_body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            );
            request.respond(response).unwrap();
            fields
        });
        (base, handle)
    }

    fn options_for(base: &str, home: &std::path::Path, timeout: Duration) -> GithubConnectOptions {
        GithubConnectOptions {
            base_urls: AuthBaseUrls::resolve(Some(base)),
            account_access_token: "account-jwt".to_string(),
            forall_home: home.to_path_buf(),
            store_mode: AuthCredentialsStoreMode::File,
            keyring_kind: AuthKeyringBackendKind::default(),
            open_browser: false,
            timeout,
        }
    }

    fn auth_url_query(auth_url: &str) -> HashMap<String, String> {
        Url::parse(auth_url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn full_connect_flow_redeems_code_and_persists_connection() {
        let home = tempfile::tempdir().unwrap();
        let (base, service) = spawn_mock_auth_service();
        let started =
            GithubConnectFlow::start(options_for(&base, home.path(), Duration::from_secs(10)))
                .await
                .unwrap();
        assert!(
            started
                .auth_url
                .starts_with("https://github.com/login/oauth/authorize")
        );
        let query = auth_url_query(&started.auth_url);
        let redirect_uri = query.get("redirect_uri").unwrap().clone();
        let state = query.get("state").unwrap().clone();
        assert!(redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(redirect_uri.ends_with("/callback"));

        // Simulate the browser being redirected back to the loopback server.
        // The blocking client lives and dies inside spawn_blocking; dropping
        // it on the async thread would panic.
        let callback = format!("{redirect_uri}?code=fake-connect-code&state={state}");
        let (status, body) = tokio::task::spawn_blocking(move || {
            let response = reqwest::blocking::get(callback).unwrap();
            (response.status(), response.text().unwrap())
        })
        .await
        .unwrap();
        assert_eq!(status, reqwest::StatusCode::OK);
        assert!(body.contains("GitHub is connected"));

        let connection = started.completion.await.unwrap().unwrap();
        assert_eq!(connection.access_token, "gho_test_token");
        assert_eq!(connection.login, "octocat");
        assert_eq!(connection.scopes, vec!["repo", "read:org", "workflow"]);

        // The mock token endpoint saw the PKCE redemption.
        let fields = service.join().unwrap();
        assert_eq!(fields.get("code").unwrap(), "fake-connect-code");
        assert_eq!(fields.get("redirect_uri").unwrap(), &redirect_uri);
        assert_eq!(fields.get("code_verifier").unwrap().len(), 43);

        // The connection is persisted and loadable.
        let loaded = load_github_connection(
            home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.access_token, "gho_test_token");
        assert_eq!(loaded.login, "octocat");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_unblocks_the_connect_flow() {
        let home = tempfile::tempdir().unwrap();
        let (base, _service) = spawn_mock_auth_service();
        let started =
            GithubConnectFlow::start(options_for(&base, home.path(), Duration::from_secs(30)))
                .await
                .unwrap();

        started.cancel.cancel();
        let err = started.completion.await.unwrap().unwrap_err();
        assert!(matches!(err, GithubConnectError::Cancelled));
        assert!(
            load_github_connection(
                home.path(),
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )
            .unwrap()
            .is_none()
        );
    }

    fn test_connection() -> GithubConnection {
        GithubConnection {
            access_token: "gho_test_token".to_string(),
            login: "octocat".to_string(),
            scopes: vec!["repo".to_string()],
            connected_at: Utc::now(),
        }
    }

    #[test]
    fn credential_get_answers_only_https_github() {
        let connection = test_connection();
        let mut out = Vec::new();
        respond_to_git_credential_request(
            "get",
            "protocol=https\nhost=github.com\n",
            Some(&connection),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "username=x-access-token\npassword=gho_test_token\n"
        );

        for (operation, input) in [
            ("get", "protocol=https\nhost=gitlab.com\n"),
            ("get", "protocol=http\nhost=github.com\n"),
            // Values after the blank line are a different (smuggled) request.
            ("get", "protocol=https\n\nhost=github.com\n"),
            ("store", "protocol=https\nhost=github.com\n"),
            ("erase", "protocol=https\nhost=github.com\n"),
        ] {
            let mut out = Vec::new();
            respond_to_git_credential_request(operation, input, Some(&connection), &mut out)
                .unwrap();
            assert!(
                out.is_empty(),
                "{operation} {input:?} must not receive credentials"
            );
        }

        let mut out = Vec::new();
        respond_to_git_credential_request(
            "get",
            "protocol=https\nhost=github.com\n",
            None,
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unauthorized_start_reports_not_signed_in() {
        let home = tempfile::tempdir().unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            let request = server.recv().unwrap();
            request
                .respond(tiny_http::Response::from_string("{}").with_status_code(401))
                .unwrap();
        });

        let err =
            match GithubConnectFlow::start(options_for(&base, home.path(), Duration::from_secs(5)))
                .await
            {
                Ok(_) => panic!("start should fail on 401"),
                Err(err) => err,
            };
        assert!(matches!(err, GithubConnectError::NotSignedIn));
    }
}
