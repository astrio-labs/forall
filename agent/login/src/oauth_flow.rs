//! Loopback OAuth authorization-code + PKCE login flow.
//!
//! [`OauthLoginFlow::start`] binds an ephemeral loopback HTTP server, builds the
//! authorization URL for the Forall auth service, and waits (on a detached
//! background thread) for the browser to be redirected back to
//! `http://127.0.0.1:{port}/callback`. The received code is exchanged at the
//! token endpoint and the resulting tokens are persisted with
//! [`crate::save_oauth_tokens`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use url::Url;

use crate::auth::AuthBaseUrls;
use crate::auth::AuthKeyringBackendKind;
use crate::auth::save_oauth_tokens;
use crate::token_data::TokenData;
use crate::token_data::parse_chatgpt_jwt_claims;
use forall_config::types::AuthCredentialsStoreMode;

/// Default overall timeout for the login flow.
pub const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

const CALLBACK_PATH: &str = "/callback";
const SUCCESS_HTML: &str = include_str!("assets/success.html");
const ERROR_HTML: &str = include_str!("assets/error.html");

/// PKCE verifier/challenge pair (RFC 7636, `S256`).
#[derive(Debug, Clone)]
pub struct Pkce {
    /// 43-character base64url (no padding) encoding of 32 random bytes.
    pub verifier: String,
    /// base64url (no padding) of `SHA-256(verifier)`.
    pub challenge: String,
}

/// Generates a fresh PKCE verifier and its `S256` challenge.
pub fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    Pkce {
        verifier,
        challenge,
    }
}

/// Identity provider requested for a managed Forall OAuth login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OauthLoginProvider {
    /// The auth service's default provider (Google today).
    #[default]
    Default,
    /// Sign in with GitHub.
    Github,
}

impl OauthLoginProvider {
    /// Value of the `provider` query parameter; `None` lets the auth service
    /// apply its default.
    fn query_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Github => Some("github"),
        }
    }
}

/// Options for [`OauthLoginFlow::start`].
#[derive(Debug, Clone)]
pub struct OauthLoginOptions {
    pub base_urls: AuthBaseUrls,
    pub forall_home: PathBuf,
    pub store_mode: AuthCredentialsStoreMode,
    pub keyring_kind: AuthKeyringBackendKind,
    /// When true, attempt to open the authorization URL in the default
    /// browser. Failures degrade gracefully; callers should always print
    /// [`StartedLogin::auth_url`] as a fallback.
    pub open_browser: bool,
    /// Overall deadline for the flow (waiting for the browser redirect plus
    /// the token exchange). Defaults to [`DEFAULT_LOGIN_TIMEOUT`].
    pub timeout: Duration,
    /// Which identity provider the auth service should send the browser to.
    pub provider: OauthLoginProvider,
}

impl OauthLoginOptions {
    pub fn new(
        base_urls: AuthBaseUrls,
        forall_home: PathBuf,
        store_mode: AuthCredentialsStoreMode,
        keyring_kind: AuthKeyringBackendKind,
    ) -> Self {
        Self {
            base_urls,
            forall_home,
            store_mode,
            keyring_kind,
            open_browser: true,
            timeout: DEFAULT_LOGIN_TIMEOUT,
            provider: OauthLoginProvider::Default,
        }
    }
}

/// Successful completion of the login flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOutcome {
    /// Tokens that were persisted via [`crate::save_oauth_tokens`].
    pub tokens: TokenData,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("timed out waiting for the browser login to complete")]
    TimedOut,
    #[error("login was cancelled")]
    Cancelled,
    #[error("state parameter returned by the auth service did not match")]
    StateMismatch,
    #[error("callback request did not include an authorization code")]
    MissingCode,
    #[error("failed to exchange the authorization code: {0}")]
    Exchange(String),
    #[error("failed to persist login credentials: {0}")]
    Persist(String),
    #[error("loopback server error: {0}")]
    Server(String),
}

/// Cancels a running [`OauthLoginFlow`] (or the GitHub connect flow, which
/// shares the same loopback mechanics), unblocking the loopback server.
#[derive(Clone)]
pub struct CancelHandle {
    cancelled: Arc<Mutex<bool>>,
    server: Arc<tiny_http::Server>,
}

impl CancelHandle {
    pub(crate) fn new(cancelled: Arc<Mutex<bool>>, server: Arc<tiny_http::Server>) -> Self {
        Self { cancelled, server }
    }

    /// Requests cancellation of the login flow.
    ///
    /// Cancellation is serialized against token persistence: the flow worker
    /// calls [`crate::auth::save_oauth_tokens`] only while holding the same
    /// mutex this method locks to set the flag, and it re-checks the flag
    /// inside that critical section. This guarantees that:
    ///
    /// - after `cancel()` returns, the flow can no longer persist tokens and
    ///   resolves its completion with [`LoginError::Cancelled`];
    /// - a save that already started when `cancel()` was called completes
    ///   before `cancel()` returns, and the flow then resolves successfully
    ///   with the persisted tokens.
    ///
    /// In other words, the completion outcome and the on-disk/keyring state
    /// never disagree, no matter how a cancel races a browser callback.
    pub fn cancel(&self) {
        *lock_cancelled(&self.cancelled) = true;
        self.server.unblock();
    }
}

/// Locks the shared cancelled flag, recovering from a poisoned mutex (the
/// protected value is a plain `bool`, so a panic in another holder cannot
/// leave it in a broken state).
pub(crate) fn lock_cancelled(cancelled: &Mutex<bool>) -> MutexGuard<'_, bool> {
    cancelled.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A started login flow.
///
/// The flow runs on a detached background OS thread; `completion` resolves
/// exactly once with the outcome. Use `completion.await` from async code or
/// `completion.blocking_recv()` from sync code.
pub struct StartedLogin {
    /// URL the user's browser must visit to authorize the login.
    pub auth_url: String,
    pub completion: tokio::sync::oneshot::Receiver<Result<LoginOutcome, LoginError>>,
    pub cancel: CancelHandle,
}

/// Loopback authorization-code + PKCE login flow.
pub struct OauthLoginFlow;

impl OauthLoginFlow {
    /// Binds the loopback server, spawns the background worker, and returns
    /// the authorization URL plus a completion channel.
    pub fn start(opts: OauthLoginOptions) -> anyhow::Result<StartedLogin> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|err| anyhow::anyhow!("failed to bind loopback login server: {err}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| anyhow::anyhow!("loopback login server has no IP address"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

        let pkce = generate_pkce();
        let state = generate_state();

        let mut auth_url =
            crate::auth::default_client::credential_endpoint(&opts.base_urls.authorize_url())
                .map_err(|err| anyhow::anyhow!("invalid authorize URL: {err}"))?;
        {
            let mut pairs = auth_url.query_pairs_mut();
            pairs
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("state", &state)
                .append_pair("code_challenge", &pkce.challenge)
                .append_pair("code_challenge_method", "S256");
            if let Some(provider) = opts.provider.query_value() {
                pairs.append_pair("provider", provider);
            }
        }
        let auth_url = auth_url.to_string();

        let server = Arc::new(server);
        let cancelled = Arc::new(Mutex::new(false));
        let cancel = CancelHandle {
            cancelled: Arc::clone(&cancelled),
            server: Arc::clone(&server),
        };

        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let worker = FlowWorker {
            server,
            cancelled,
            state,
            pkce_verifier: pkce.verifier,
            redirect_uri,
            token_url: opts.base_urls.token_url(),
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
            && let Err(err) = webbrowser::open(&auth_url)
        {
            tracing::warn!("failed to open browser for login: {err}");
        }

        Ok(StartedLogin {
            auth_url,
            completion: completion_rx,
            cancel,
        })
    }
}

pub(crate) fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct FlowWorker {
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

impl FlowWorker {
    fn is_cancelled(&self) -> bool {
        *lock_cancelled(&self.cancelled)
    }

    fn run(&self) -> Result<LoginOutcome, LoginError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.is_cancelled() {
                return Err(LoginError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(LoginError::TimedOut);
            }
            let request = match self.server.recv_timeout(deadline - now) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    // `recv_timeout` returns `None` both on timeout and when
                    // `CancelHandle::cancel` unblocks the server.
                    if self.is_cancelled() {
                        return Err(LoginError::Cancelled);
                    }
                    return Err(LoginError::TimedOut);
                }
                Err(err) => {
                    if self.is_cancelled() {
                        return Err(LoginError::Cancelled);
                    }
                    return Err(LoginError::Server(err.to_string()));
                }
            };

            // A cancel may already have raced this callback into the server's
            // queue; bail out cheaply before starting the token exchange.
            if self.is_cancelled() {
                respond(request, cancelled_response());
                return Err(LoginError::Cancelled);
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
                let err = LoginError::StateMismatch;
                respond(request, login_error_response(&err, 400));
                return Err(err);
            }
            let Some(code) = code else {
                let err = LoginError::MissingCode;
                respond(request, login_error_response(&err, 400));
                return Err(err);
            };

            // Exchange the code first (network I/O, no lock held), then
            // decide atomically below whether the tokens may be persisted.
            let tokens = match self.exchange_code(&code) {
                Ok(tokens) => tokens,
                Err(err) => {
                    respond(request, login_error_response(&err, 500));
                    return Err(err);
                }
            };

            // Cancel-vs-persist critical section: `CancelHandle::cancel` sets
            // the flag while holding this same mutex, so once `cancel()` has
            // returned no save can start, and a save that already started
            // completes before `cancel()` returns.
            //
            // `save_oauth_tokens` performs file/keyring I/O while the mutex is
            // held. That is acceptable here: each login flow has exactly one
            // worker thread and the only other contender is a single
            // `CancelHandle::cancel` call.
            let cancelled_guard = lock_cancelled(&self.cancelled);
            if *cancelled_guard {
                drop(cancelled_guard);
                respond(request, cancelled_response());
                return Err(LoginError::Cancelled);
            }
            let save_result = save_oauth_tokens(
                &self.forall_home,
                tokens.clone(),
                self.store_mode,
                self.keyring_kind,
            )
            .map_err(|err| LoginError::Persist(err.to_string()));
            drop(cancelled_guard);

            return match save_result {
                Ok(()) => {
                    respond(request, html_response(SUCCESS_HTML));
                    Ok(LoginOutcome { tokens })
                }
                Err(err) => {
                    respond(request, login_error_response(&err, 500));
                    Err(err)
                }
            };
        }
    }

    fn exchange_code(&self, code: &str) -> Result<TokenData, LoginError> {
        #[derive(Deserialize)]
        struct TokenResponse {
            id_token: String,
            access_token: String,
            refresh_token: String,
        }

        let response = crate::auth::default_client::blocking_credential_post(&self.token_url)
            .map_err(|err| LoginError::Exchange(err.to_string()))?
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", self.pkce_verifier.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .send()
            .map_err(|err| LoginError::Exchange(err.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(LoginError::Exchange(format!(
                "token endpoint returned HTTP {status}: {body}"
            )));
        }
        let token_response: TokenResponse = response
            .json()
            .map_err(|err| LoginError::Exchange(format!("invalid token response: {err}")))?;

        let id_token = parse_chatgpt_jwt_claims(&token_response.id_token)
            .map_err(|err| LoginError::Exchange(format!("invalid id token: {err}")))?;
        let account_id = id_token.chatgpt_account_id.clone();
        Ok(TokenData {
            id_token,
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            account_id,
        })
    }
}

pub(crate) type HttpResponse = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

pub(crate) fn html_response(body: &str) -> HttpResponse {
    with_content_type(
        tiny_http::Response::from_string(body),
        "text/html; charset=utf-8",
    )
}

/// Renders the branded error page with the given title and detail text.
///
/// Both values are HTML-escaped: exchange failures embed the token endpoint's
/// response body verbatim, which must not be interpreted as markup.
pub(crate) fn error_page_response(title: &str, detail: &str, status: u16) -> HttpResponse {
    with_content_type(
        tiny_http::Response::from_string(error_page_body(title, detail)).with_status_code(status),
        "text/html; charset=utf-8",
    )
}

fn error_page_body(title: &str, detail: &str) -> String {
    ERROR_HTML
        .replace("{{error_title}}", &html_escape(title))
        .replace("{{error_detail}}", &html_escape(detail))
}

fn login_error_response(err: &LoginError, status: u16) -> HttpResponse {
    error_page_response(
        "Sign-in failed",
        &format!("{err}. You can close this tab and try again from Forall."),
        status,
    )
}

fn cancelled_response() -> HttpResponse {
    error_page_response(
        "Login cancelled",
        "This sign-in was cancelled from Forall. You can close this tab.",
        200,
    )
}

fn html_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub(crate) fn plain_response(body: &str, status: u16) -> HttpResponse {
    with_content_type(
        tiny_http::Response::from_string(body).with_status_code(status),
        "text/plain; charset=utf-8",
    )
}

fn with_content_type(response: HttpResponse, content_type: &str) -> HttpResponse {
    match tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        Ok(header) => response.with_header(header),
        Err(()) => response,
    }
}

pub(crate) fn respond(request: tiny_http::Request, response: HttpResponse) {
    if let Err(err) = request.respond(response) {
        tracing::warn!("failed to respond to login callback request: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::load_auth_dot_json;
    use forall_app_server_protocol::AuthMode;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    fn fake_jwt(payload: serde_json::Value) -> String {
        let header_b64 = URL_SAFE_NO_PAD.encode(b"{}");
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let signature_b64 = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header_b64}.{payload_b64}.{signature_b64}")
    }

    fn fake_id_token() -> String {
        fake_jwt(serde_json::json!({
            "email": "user@example.com",
            "https://forall.astrio.app/auth": {
                "chatgpt_user_id": "user-1",
                "chatgpt_account_id": "account-1",
                "chatgpt_plan_type": "pro",
            },
        }))
    }

    /// Serves exactly one token-exchange request, then returns the parsed
    /// form fields.
    fn spawn_mock_token_server() -> (String, std::thread::JoinHandle<HashMap<String, String>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            assert_eq!(request.method(), &tiny_http::Method::Post);
            assert_eq!(request.url(), "/oauth/token");
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let fields: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let response_body = serde_json::json!({
                "id_token": fake_id_token(),
                "access_token": fake_id_token(),
                "refresh_token": "refresh-1",
                "token_type": "bearer",
                "expires_in": 3600,
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

    fn options_for(base: &str, home: &std::path::Path, timeout: Duration) -> OauthLoginOptions {
        OauthLoginOptions {
            base_urls: AuthBaseUrls::resolve(Some(base)),
            forall_home: home.to_path_buf(),
            store_mode: AuthCredentialsStoreMode::File,
            keyring_kind: AuthKeyringBackendKind::default(),
            open_browser: false,
            timeout,
            provider: OauthLoginProvider::Default,
        }
    }

    fn auth_url_query(auth_url: &str) -> HashMap<String, String> {
        Url::parse(auth_url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn github_provider_lands_in_the_authorize_url_and_default_stays_absent() {
        let home = tempfile::tempdir().unwrap();
        let mut opts = options_for("http://127.0.0.1:9", home.path(), Duration::from_secs(5));
        opts.provider = OauthLoginProvider::Github;
        let started = OauthLoginFlow::start(opts).unwrap();
        assert_eq!(
            auth_url_query(&started.auth_url)
                .get("provider")
                .map(String::as_str),
            Some("github")
        );
        started.cancel.cancel();

        let default = OauthLoginFlow::start(options_for(
            "http://127.0.0.1:9",
            home.path(),
            Duration::from_secs(5),
        ))
        .unwrap();
        assert!(!auth_url_query(&default.auth_url).contains_key("provider"));
        default.cancel.cancel();
    }

    #[test]
    fn generate_pkce_produces_valid_pair() {
        let pkce = generate_pkce();
        assert_eq!(pkce.verifier.len(), 43);
        assert!(pkce.verifier.len() >= 43 && pkce.verifier.len() <= 128);
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()));
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn full_flow_exchanges_code_and_persists_tokens() {
        let home = tempfile::tempdir().unwrap();
        let (base, token_server) = spawn_mock_token_server();
        let started =
            OauthLoginFlow::start(options_for(&base, home.path(), Duration::from_secs(10)))
                .unwrap();

        assert!(
            started
                .auth_url
                .starts_with(&format!("{base}/oauth/authorize?"))
        );
        let query = auth_url_query(&started.auth_url);
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(query.get("code_challenge").unwrap().len(), 43);
        let state = query.get("state").unwrap().clone();
        let redirect_uri = query.get("redirect_uri").unwrap().clone();
        assert!(redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(redirect_uri.ends_with("/callback"));

        // Simulate the browser being redirected back to the loopback server.
        let callback = format!("{redirect_uri}?code=fake-code&state={state}");
        let response = reqwest::blocking::get(callback).unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(response.text().unwrap().contains("Signed in to Forall"));

        let outcome = started.completion.blocking_recv().unwrap().unwrap();
        assert_eq!(outcome.tokens.refresh_token, "refresh-1");
        assert_eq!(outcome.tokens.account_id.as_deref(), Some("account-1"));

        // The mock token endpoint saw the PKCE exchange.
        let fields = token_server.join().unwrap();
        assert_eq!(fields.get("grant_type").unwrap(), "authorization_code");
        assert_eq!(fields.get("code").unwrap(), "fake-code");
        assert_eq!(fields.get("redirect_uri").unwrap(), &redirect_uri);
        assert_eq!(fields.get("code_verifier").unwrap().len(), 43);

        // auth.json now holds the tokens and loads as OAuth auth.
        let auth_dot_json = load_auth_dot_json(
            home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(auth_dot_json.auth_mode, Some(AuthMode::Oauth));
        let tokens = auth_dot_json.tokens.unwrap();
        assert_eq!(tokens.refresh_token, "refresh-1");
        assert_eq!(tokens.id_token.email.as_deref(), Some("user@example.com"));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let auth = runtime
            .block_on(crate::auth::ForallAuth::from_auth_storage(
                home.path(),
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
                None,
            ))
            .unwrap()
            .unwrap();
        assert!(matches!(auth, crate::auth::ForallAuth::Oauth(_)));
        assert_eq!(
            auth.get_account_email().as_deref(),
            Some("user@example.com")
        );
    }

    #[test]
    fn state_mismatch_fails_the_flow() {
        let home = tempfile::tempdir().unwrap();
        let started = OauthLoginFlow::start(options_for(
            "http://127.0.0.1:9",
            home.path(),
            Duration::from_secs(10),
        ))
        .unwrap();

        let query = auth_url_query(&started.auth_url);
        let redirect_uri = query.get("redirect_uri").unwrap().clone();
        let callback = format!("{redirect_uri}?code=fake-code&state=wrong-state");
        let response = reqwest::blocking::get(callback).unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = response.text().unwrap();
        assert!(body.contains("Sign-in failed"));
        assert!(!body.contains("{{error_title}}"));

        let err = started.completion.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, LoginError::StateMismatch));
    }

    #[test]
    fn error_page_escapes_html_in_details() {
        let body = error_page_body("Sign-in failed", "<script>alert('x')</script> & \"quotes\"");
        assert!(!body.contains("<script>"));
        assert!(
            body.contains(
                "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;quotes&quot;"
            )
        );
        assert!(!body.contains("{{error_title}}"));
        assert!(!body.contains("{{error_detail}}"));
    }

    #[test]
    fn times_out_without_a_callback() {
        let home = tempfile::tempdir().unwrap();
        let started = OauthLoginFlow::start(options_for(
            "http://127.0.0.1:9",
            home.path(),
            Duration::from_millis(100),
        ))
        .unwrap();

        let err = started.completion.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, LoginError::TimedOut));
    }

    #[test]
    fn cancel_unblocks_the_flow() {
        let home = tempfile::tempdir().unwrap();
        let started = OauthLoginFlow::start(options_for(
            "http://127.0.0.1:9",
            home.path(),
            Duration::from_secs(30),
        ))
        .unwrap();

        started.cancel.cancel();
        let err = started.completion.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, LoginError::Cancelled));
    }

    fn persisted_tokens(home: &std::path::Path) -> bool {
        load_auth_dot_json(
            home,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .unwrap()
        .and_then(|auth_dot_json| auth_dot_json.tokens)
        .is_some()
    }

    #[test]
    fn cancel_before_callback_leaves_no_tokens() {
        let home = tempfile::tempdir().unwrap();
        // Real token endpoint so a (buggy) exchange would actually succeed.
        let (base, _token_server) = spawn_mock_token_server();
        let started =
            OauthLoginFlow::start(options_for(&base, home.path(), Duration::from_secs(10)))
                .unwrap();

        started.cancel.cancel();
        let err = started.completion.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, LoginError::Cancelled));
        assert!(!persisted_tokens(home.path()));
    }

    /// Property-style check of the cancel-vs-persist invariant: for every
    /// interleaving of a browser callback with `CancelHandle::cancel`, the
    /// completion outcome and the persisted state must agree — success iff
    /// tokens were saved, cancelled implies nothing was saved.
    #[test]
    fn concurrent_cancel_and_callback_agree_with_persisted_state() {
        for iteration in 0..20 {
            let home = tempfile::tempdir().unwrap();
            let (base, _token_server) = spawn_mock_token_server();
            let started =
                OauthLoginFlow::start(options_for(&base, home.path(), Duration::from_secs(10)))
                    .unwrap();

            let query = auth_url_query(&started.auth_url);
            let state = query.get("state").unwrap().clone();
            let redirect_uri = query.get("redirect_uri").unwrap().clone();
            let callback = format!("{redirect_uri}?code=fake-code&state={state}");

            let cancel = started.cancel;
            let callback_thread = std::thread::spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap();
                // The worker may already have exited due to cancellation; the
                // HTTP outcome of the callback is irrelevant to the invariant.
                let _ = client.get(callback).send();
            });
            let cancel_thread = std::thread::spawn(move || {
                cancel.cancel();
            });
            let result = started.completion.blocking_recv().unwrap();
            callback_thread.join().unwrap();
            cancel_thread.join().unwrap();

            let tokens_persisted = persisted_tokens(home.path());
            match result {
                Ok(_) => assert!(
                    tokens_persisted,
                    "iteration {iteration}: success outcome without persisted tokens"
                ),
                Err(LoginError::Cancelled) => assert!(
                    !tokens_persisted,
                    "iteration {iteration}: cancelled outcome but tokens were persisted"
                ),
                Err(other) => panic!("iteration {iteration}: unexpected outcome: {other}"),
            }
        }
    }
}
