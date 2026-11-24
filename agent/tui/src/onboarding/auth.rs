//! Authentication step UI and state transitions used by onboarding.
//!
//! This module owns the BYOK auth-step state machine, renders the corresponding UI,
//! and handles auth-scoped keyboard input. It intentionally does not decide onboarding
//! flow completion; the enclosing onboarding screen coordinates step progression.

#![allow(clippy::unwrap_used)]

use crate::config_update;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use forall_app_server_client::AppServerRequestHandle;
use forall_app_server_protocol::AccountLoginCompletedNotification;
use forall_app_server_protocol::AccountUpdatedNotification;
use forall_app_server_protocol::CancelLoginAccountParams;
use forall_app_server_protocol::CancelLoginAccountResponse;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::LoginAccountParams;
use forall_app_server_protocol::LoginAccountResponse;
use forall_login::AuthKeyringBackendKind;
use forall_login::load_byok_env_files;
use forall_login::read_anthropic_api_key_from_env;
use forall_login::read_azure_openai_api_key_from_env;
use forall_login::read_gemini_api_key_from_env;
use forall_login::read_openai_api_key_from_env;
use forall_login::read_openrouter_api_key_from_env;
use forall_login::read_stored_provider_api_key_from_disk;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use forall_config::types::AuthCredentialsStoreMode;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use crate::LoginStatus;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::onboarding::keys;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::style::AccentStylize;
use crate::style::primary_accent_fg;
use crate::tui::FrameRequester;

/// Marks buffer cells that have cyan+underlined style as an OSC 8 hyperlink.
///
/// Terminal emulators recognise the OSC 8 escape sequence and treat the entire
/// marked region as a single clickable link, regardless of row wrapping.  This
/// is necessary because ratatui's cell-based rendering emits `MoveTo` at every
/// row boundary, which breaks normal terminal URL detection for long URLs that
/// wrap across multiple rows.
pub(crate) fn mark_url_hyperlink(buf: &mut Buffer, area: Rect, url: &str) {
    crate::terminal_hyperlinks::mark_url_hyperlink(buf, area, url);
}

/// Marks any underlined buffer cells as an OSC 8 hyperlink.
pub(crate) fn mark_underlined_hyperlink(buf: &mut Buffer, area: Rect, url: &str) {
    crate::terminal_hyperlinks::mark_underlined_hyperlink(buf, area, url);
}

use super::onboarding_screen::StepState;

#[derive(Clone)]
pub(crate) enum SignInState {
    PickAuthMode,
    /// `account/login/start` has been sent but the response carrying the
    /// `login_id` has not arrived yet. `attempt` pairs the eventual response
    /// with the attempt that issued it. `cancel_requested` is set when the
    /// user presses Esc during this window: the UI returns to the auth-mode
    /// picker immediately, and when the response finally arrives its
    /// `login_id` is cancelled right away instead of entering the waiting
    /// state (otherwise the server-side flow would stay alive and a browser
    /// callback could still log the user in).
    ForallOauthStarting {
        attempt: u64,
        cancel_requested: bool,
    },
    /// A managed Forall OAuth login is in flight. `auth_url` and `login_id`
    /// are populated once the app server responds to `account/login/start`.
    ForallOauth(ForallOauthState),
    ForallOauthComplete,
    PickByokProvider,
    ApiKeyEntry(ApiKeyInputState),
    ApiKeyConfigured,
}

#[derive(Clone, Default)]
pub(crate) struct ForallOauthState {
    auth_url: Option<String>,
    login_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum AuthModeChoice {
    #[default]
    Forall,
    Byok,
}

impl AuthModeChoice {
    const ALL: [Self; 2] = [Self::Forall, Self::Byok];

    fn display_name(self) -> &'static str {
        match self {
            Self::Forall => "Forall account",
            Self::Byok => "Bring your own API key",
        }
    }

    fn setup_blurb(self) -> &'static str {
        match self {
            Self::Forall => "Sign in with your browser to use your Forall account",
            Self::Byok => {
                "Use OpenAI, OpenRouter, Azure OpenAI, Anthropic, or Gemini keys from ~/.forall/.env"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum ByokProvider {
    #[default]
    OpenAi,
    OpenRouter,
    Azure,
    Anthropic,
    Gemini,
}

impl ByokProvider {
    fn model_provider_id(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Azure => "azure",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::OpenRouter => "OpenRouter",
            Self::Azure => "Azure OpenAI",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Google Gemini",
        }
    }

    fn env_var_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Azure => "AZURE_OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
        }
    }

    fn setup_blurb(self) -> &'static str {
        match self {
            Self::OpenAi => "Direct access to OpenAI models via OPENAI_API_KEY in .env",
            Self::OpenRouter => "Hundreds of models via OPENROUTER_API_KEY in .env",
            Self::Azure => "Azure OpenAI via AZURE_OPENAI_API_KEY and AZURE_OPENAI_BASE_URL",
            Self::Anthropic => "Claude models via ANTHROPIC_API_KEY in .env",
            Self::Gemini => "Gemini models via GEMINI_API_KEY in .env",
        }
    }

    fn read_from_env(self) -> Option<String> {
        match self {
            Self::OpenAi => read_openai_api_key_from_env(),
            Self::OpenRouter => read_openrouter_api_key_from_env(),
            Self::Azure => read_azure_openai_api_key_from_env(),
            Self::Anthropic => read_anthropic_api_key_from_env(),
            Self::Gemini => read_gemini_api_key_from_env(),
        }
    }

    const ALL: [Self; 5] = [
        Self::OpenAi,
        Self::OpenRouter,
        Self::Azure,
        Self::Anthropic,
        Self::Gemini,
    ];
}

fn onboarding_request_id() -> forall_app_server_protocol::RequestId {
    forall_app_server_protocol::RequestId::String(uuid::Uuid::new_v4().to_string())
}

/// Monotonic id distinguishing `account/login/start` attempts, so a late
/// response from an abandoned attempt is never adopted by (or allowed to
/// clobber) a newer one.
static NEXT_LOGIN_START_ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// What the login-start task must do after applying the
/// `account/login/start` response to the sign-in state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LoginStartAction {
    /// The attempt is still current: open the authorization URL and wait for
    /// `account/login/completed`.
    OpenBrowser(String),
    /// The attempt was abandoned (Esc during pending-start, a newer attempt,
    /// or the user navigated elsewhere): the server-side flow is still alive,
    /// so cancel `login_id` immediately so a late browser callback can no
    /// longer complete the login.
    CancelLogin(String),
}

/// Applies a successful `account/login/start` response for `attempt` to the
/// sign-in state and returns the follow-up action for the caller.
fn apply_login_start_response(
    sign_in_state: &RwLock<SignInState>,
    attempt: u64,
    login_id: String,
    auth_url: String,
) -> LoginStartAction {
    let mut guard = sign_in_state.write().unwrap();
    match &*guard {
        SignInState::ForallOauthStarting {
            attempt: pending,
            cancel_requested: false,
        } if *pending == attempt => {
            *guard = SignInState::ForallOauth(ForallOauthState {
                auth_url: Some(auth_url.clone()),
                login_id: Some(login_id),
            });
            LoginStartAction::OpenBrowser(auth_url)
        }
        SignInState::ForallOauthStarting {
            attempt: pending,
            cancel_requested: true,
        } if *pending == attempt => {
            *guard = SignInState::PickAuthMode;
            LoginStartAction::CancelLogin(login_id)
        }
        // The widget moved on (a newer attempt, BYOK, ...): nothing tracks
        // this login any more, so cancel it server-side and leave the current
        // state untouched.
        _ => LoginStartAction::CancelLogin(login_id),
    }
}

/// Returns the sign-in state to the auth-mode picker if the failed start
/// still belongs to `attempt`. Reports whether an error message should be
/// surfaced (suppressed when the user already cancelled the attempt).
fn reset_failed_login_start(sign_in_state: &RwLock<SignInState>, attempt: u64) -> bool {
    let mut guard = sign_in_state.write().unwrap();
    match &*guard {
        SignInState::ForallOauthStarting {
            attempt: pending,
            cancel_requested,
        } if *pending == attempt => {
            let show_error = !*cancel_requested;
            *guard = SignInState::PickAuthMode;
            show_error
        }
        _ => false,
    }
}

#[derive(Clone, Default)]
pub(crate) struct ApiKeyInputState {
    value: String,
    prepopulated_from_env: bool,
    provider: ByokProvider,
}

impl KeyboardHandler for AuthModeWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if self.handle_api_key_entry_key_event(&key_event) {
            return;
        }

        if keys::MOVE_UP.is_pressed(key_event) {
            self.move_highlight(/*delta*/ -1);
            return;
        }
        if keys::MOVE_DOWN.is_pressed(key_event) {
            self.move_highlight(/*delta*/ 1);
            return;
        }
        if keys::SELECT_FIRST.is_pressed(key_event) {
            let state = { (*self.sign_in_state.read().unwrap()).clone() };
            match state {
                SignInState::PickAuthMode
                | SignInState::ForallOauthStarting {
                    cancel_requested: true,
                    ..
                } => {
                    self.highlighted_auth_mode = AuthModeChoice::ALL[0];
                    self.request_frame.schedule_frame();
                }
                SignInState::PickByokProvider => {
                    self.highlighted_byok_provider = ByokProvider::ALL[0];
                    self.request_frame.schedule_frame();
                }
                _ => {}
            }
            return;
        }
        if keys::SELECT_SECOND.is_pressed(key_event) {
            let state = { (*self.sign_in_state.read().unwrap()).clone() };
            match state {
                SignInState::PickAuthMode
                | SignInState::ForallOauthStarting {
                    cancel_requested: true,
                    ..
                } => {
                    self.highlighted_auth_mode = AuthModeChoice::ALL[1];
                    self.request_frame.schedule_frame();
                }
                SignInState::PickByokProvider => {
                    self.highlighted_byok_provider = ByokProvider::ALL[1];
                    self.request_frame.schedule_frame();
                }
                _ => {}
            }
            return;
        }
        if keys::SELECT_THIRD.is_pressed(key_event) {
            let state = { (*self.sign_in_state.read().unwrap()).clone() };
            // Only the BYOK provider picker has a third entry; the auth-mode
            // picker offers two choices.
            if matches!(state, SignInState::PickByokProvider) {
                self.highlighted_byok_provider = ByokProvider::ALL[2];
                self.request_frame.schedule_frame();
            }
            return;
        }
        if keys::SELECT_FOURTH.is_pressed(key_event) {
            let state = { (*self.sign_in_state.read().unwrap()).clone() };
            // Only the BYOK provider picker has a fourth entry.
            if matches!(state, SignInState::PickByokProvider) {
                self.highlighted_byok_provider = ByokProvider::ALL[3];
                self.request_frame.schedule_frame();
            }
            return;
        }
        if keys::SELECT_FIFTH.is_pressed(key_event) {
            let state = { (*self.sign_in_state.read().unwrap()).clone() };
            // Only the BYOK provider picker has a fifth entry.
            if matches!(state, SignInState::PickByokProvider) {
                self.highlighted_byok_provider = ByokProvider::ALL[4];
                self.request_frame.schedule_frame();
            }
            return;
        }
        if keys::CONFIRM.is_pressed(key_event) {
            let sign_in_state = { (*self.sign_in_state.read().unwrap()).clone() };
            match sign_in_state {
                SignInState::PickAuthMode
                | SignInState::ForallOauthStarting {
                    cancel_requested: true,
                    ..
                } => {
                    self.confirm_auth_mode_selection();
                }
                SignInState::PickByokProvider => {
                    self.confirm_byok_provider_selection();
                }
                _ => {}
            }
            return;
        }
        if keys::CANCEL.is_pressed(key_event) {
            tracing::info!("Cancel onboarding auth step");
            self.cancel_active_attempt();
        }
    }

    fn handle_paste(&mut self, pasted: String) {
        let _ = self.handle_api_key_entry_paste(pasted);
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct AuthModeWidget {
    pub request_frame: FrameRequester,
    pub highlighted_auth_mode: AuthModeChoice,
    pub highlighted_byok_provider: ByokProvider,
    pub error: Arc<RwLock<Option<String>>>,
    pub sign_in_state: Arc<RwLock<SignInState>>,
    pub login_status: LoginStatus,
    pub app_server_request_handle: AppServerRequestHandle,
    pub animations_enabled: bool,
    pub animations_suppressed: Cell<bool>,
    pub(crate) forall_home: PathBuf,
    pub(crate) workspace_cwd: PathBuf,
    pub(crate) auth_credentials_store_mode: AuthCredentialsStoreMode,
    pub(crate) provider_env_key: Option<String>,
}

impl AuthModeWidget {
    pub(crate) fn set_animations_suppressed(&self, suppressed: bool) {
        self.animations_suppressed.set(suppressed);
    }

    pub(crate) fn should_suppress_animations(&self) -> bool {
        false
    }

    pub(crate) fn cancel_active_attempt(&self) {
        let mut sign_in_state = self.sign_in_state.write().unwrap();
        match &*sign_in_state {
            SignInState::ApiKeyEntry(_) => {
                *sign_in_state = SignInState::PickByokProvider;
            }
            SignInState::ForallOauthStarting {
                attempt,
                cancel_requested,
            } => {
                let attempt = *attempt;
                if *cancel_requested {
                    // Already marked; nothing further to do.
                    return;
                }
                // The `account/login/start` response (with the login_id to
                // cancel) has not arrived yet. Remember the cancellation so
                // the response handler cancels the login immediately instead
                // of entering the waiting state; the UI returns to the picker
                // right away.
                *sign_in_state = SignInState::ForallOauthStarting {
                    attempt,
                    cancel_requested: true,
                };
            }
            SignInState::ForallOauth(state) => {
                if let Some(login_id) = state.login_id.clone() {
                    let request_handle = self.app_server_request_handle.clone();
                    tokio::spawn(async move {
                        // Best effort: the server reports the aborted login via
                        // `account/login/completed`, but this widget has already
                        // returned to the picker by then.
                        let _ = request_handle
                            .request_typed::<CancelLoginAccountResponse>(
                                ClientRequest::CancelLoginAccount {
                                    request_id: onboarding_request_id(),
                                    params: CancelLoginAccountParams { login_id },
                                },
                            )
                            .await;
                    });
                }
                *sign_in_state = SignInState::PickAuthMode;
            }
            SignInState::PickByokProvider => {
                *sign_in_state = SignInState::PickAuthMode;
            }
            _ => return,
        }
        drop(sign_in_state);
        self.set_error(/*message*/ None);
        self.request_frame.schedule_frame();
    }

    fn set_error(&self, message: Option<String>) {
        *self.error.write().unwrap() = message;
    }

    fn error_message(&self) -> Option<String> {
        self.error.read().unwrap().clone()
    }

    /// Returns whether the auth flow is currently in API-key entry mode.
    pub(crate) fn is_api_key_entry_active(&self) -> bool {
        self.sign_in_state
            .read()
            .is_ok_and(|guard| matches!(&*guard, SignInState::ApiKeyEntry(_)))
    }

    /// Returns whether the API-key entry field currently contains any text.
    pub(crate) fn api_key_entry_has_text(&self) -> bool {
        self.sign_in_state.read().is_ok_and(|guard| match &*guard {
            SignInState::ApiKeyEntry(state) => !state.value.is_empty(),
            _ => false,
        })
    }

    fn confirm_binding(&self) -> KeyBinding {
        keys::CONFIRM[0]
    }

    fn cancel_binding(&self) -> KeyBinding {
        keys::CANCEL[0]
    }

    fn move_byok_provider_highlight(&mut self, delta: isize) {
        let options = ByokProvider::ALL;
        let current_index = options
            .iter()
            .position(|provider| *provider == self.highlighted_byok_provider)
            .unwrap_or(0);
        let next_index =
            (current_index as isize + delta).rem_euclid(options.len() as isize) as usize;
        self.highlighted_byok_provider = options[next_index];
    }

    fn move_auth_mode_highlight(&mut self, delta: isize) {
        let options = AuthModeChoice::ALL;
        let current_index = options
            .iter()
            .position(|mode| *mode == self.highlighted_auth_mode)
            .unwrap_or(0);
        let next_index =
            (current_index as isize + delta).rem_euclid(options.len() as isize) as usize;
        self.highlighted_auth_mode = options[next_index];
    }

    fn move_highlight(&mut self, delta: isize) {
        let kind = {
            let guard = self.sign_in_state.read().unwrap();
            match &*guard {
                SignInState::PickAuthMode
                | SignInState::ForallOauthStarting {
                    cancel_requested: true,
                    ..
                } => 0,
                SignInState::PickByokProvider => 1,
                _ => 2,
            }
        };
        match kind {
            0 => self.move_auth_mode_highlight(delta),
            1 => self.move_byok_provider_highlight(delta),
            _ => {}
        }
    }

    fn confirm_auth_mode_selection(&mut self) {
        self.set_error(/*message*/ None);
        match self.highlighted_auth_mode {
            AuthModeChoice::Forall => {
                self.start_forall_oauth_login();
            }
            AuthModeChoice::Byok => {
                *self.sign_in_state.write().unwrap() = SignInState::PickByokProvider;
            }
        }
        self.request_frame.schedule_frame();
    }

    /// Starts a managed Forall OAuth login through the app server and opens
    /// the returned authorization URL in the user's browser. If the user
    /// backs out (Esc) before the `account/login/start` response arrives, the
    /// returned `login_id` is cancelled immediately instead of being dropped.
    fn start_forall_oauth_login(&mut self) {
        let attempt = NEXT_LOGIN_START_ATTEMPT.fetch_add(1, Ordering::Relaxed);
        *self.sign_in_state.write().unwrap() = SignInState::ForallOauthStarting {
            attempt,
            cancel_requested: false,
        };
        let request_handle = self.app_server_request_handle.clone();
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        tokio::spawn(async move {
            match request_handle
                .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
                    request_id: onboarding_request_id(),
                    params: LoginAccountParams::Oauth { provider: None },
                })
                .await
            {
                Ok(LoginAccountResponse::Oauth { login_id, auth_url }) => {
                    match apply_login_start_response(&sign_in_state, attempt, login_id, auth_url) {
                        LoginStartAction::OpenBrowser(auth_url) => {
                            if let Err(err) = webbrowser::open(&auth_url) {
                                tracing::warn!("failed to open browser for Forall sign-in: {err}");
                            }
                        }
                        LoginStartAction::CancelLogin(login_id) => {
                            // Best effort, same path as Esc during the
                            // waiting state; the server reports the aborted
                            // login via `account/login/completed`.
                            let _ = request_handle
                                .request_typed::<CancelLoginAccountResponse>(
                                    ClientRequest::CancelLoginAccount {
                                        request_id: onboarding_request_id(),
                                        params: CancelLoginAccountParams { login_id },
                                    },
                                )
                                .await;
                        }
                    }
                }
                Ok(_) => {
                    if reset_failed_login_start(&sign_in_state, attempt) {
                        *error.write().unwrap() =
                            Some("Unexpected response to the login request".to_string());
                    }
                }
                Err(err) => {
                    if reset_failed_login_start(&sign_in_state, attempt) {
                        *error.write().unwrap() =
                            Some(format!("Failed to start Forall sign-in: {err}"));
                    }
                }
            }
            request_frame.schedule_frame();
        });
        self.request_frame.schedule_frame();
    }

    fn render_auth_mode_picker(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line> = vec!["How do you want to sign in?".bold().into(), "".into()];

        for (idx, mode) in AuthModeChoice::ALL.into_iter().enumerate() {
            let is_selected = self.highlighted_auth_mode == mode;
            let caret = if is_selected { ">" } else { " " };
            let line1 = if is_selected {
                Line::from(vec![
                    format!("{caret} {index}. ", index = idx + 1).accent().dim(),
                    mode.display_name().to_string().accent(),
                ])
            } else {
                format!("  {}. {}", idx + 1, mode.display_name()).into()
            };
            let line2 = if is_selected {
                Line::from(format!("     {}", mode.setup_blurb()))
                    .fg(primary_accent_fg())
                    .add_modifier(Modifier::DIM)
            } else {
                Line::from(format!("     {}", mode.setup_blurb()))
                    .style(Style::default().add_modifier(Modifier::DIM))
            };
            lines.extend([line1, line2, "".into()]);
        }

        lines.push(Line::from(vec![
            "  Press ".dim(),
            self.confirm_binding().into(),
            " to continue".dim(),
        ]));
        if let Some(err) = self.error_message() {
            lines.push("".into());
            lines.push(err.red().into());
        }

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_forall_oauth(&self, area: Rect, buf: &mut Buffer, state: &ForallOauthState) {
        let mut lines: Vec<Line> =
            vec!["Sign in with your Forall account".bold().into(), "".into()];
        match state.auth_url.as_deref() {
            Some(auth_url) => {
                lines.push("  Finish signing in in the browser window that just opened.".into());
                lines.push("  If it did not open, visit:".into());
                lines.push(Line::from(vec![
                    "     ".into(),
                    auth_url.to_string().cyan().underlined(),
                ]));
            }
            None => {
                lines.push("  Starting sign-in...".into());
            }
        }
        lines.push("".into());
        lines.push(Line::from(vec![
            "  Press ".dim(),
            self.cancel_binding().into(),
            " to go back".dim(),
        ]));
        if let Some(err) = self.error_message() {
            lines.push("".into());
            lines.push(err.red().into());
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
        if let Some(auth_url) = state.auth_url.as_deref() {
            mark_url_hyperlink(buf, area, auth_url);
        }
    }

    fn render_forall_oauth_complete(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ Signed in with your Forall account"
                .fg(Color::Green)
                .into(),
            "".into(),
            "  Forall will use your account for chat.".into(),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_byok_provider_picker(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line> = vec![
            "Choose your model provider".bold().into(),
            "".into(),
            "  Bring your own API key from ~/.forall/.env or a project .env file.".into(),
            "".into(),
        ];

        for (idx, provider) in ByokProvider::ALL.into_iter().enumerate() {
            let is_selected = self.highlighted_byok_provider == provider;
            let caret = if is_selected { ">" } else { " " };
            let line1 = if is_selected {
                Line::from(vec![
                    format!("{caret} {index}. ", index = idx + 1).accent().dim(),
                    provider.display_name().to_string().accent(),
                ])
            } else {
                format!("  {}. {}", idx + 1, provider.display_name()).into()
            };
            let line2 = if is_selected {
                Line::from(format!("     {}", provider.setup_blurb()))
                    .fg(primary_accent_fg())
                    .add_modifier(Modifier::DIM)
            } else {
                Line::from(format!("     {}", provider.setup_blurb()))
                    .style(Style::default().add_modifier(Modifier::DIM))
            };
            lines.extend([line1, line2, "".into()]);
        }

        lines.push(Line::from(vec![
            "  Press ".dim(),
            self.confirm_binding().into(),
            " after setting the key in .env".dim(),
        ]));
        if let Some(err) = self.error_message() {
            lines.push("".into());
            lines.push(err.red().into());
        }

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_api_key_configured(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ API key configured".fg(Color::Green).into(),
            "".into(),
            "  Forall will use your API key for chat.".into(),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_api_key_entry(&self, area: Rect, buf: &mut Buffer, state: &ApiKeyInputState) {
        let provider = state.provider;
        let [intro_area, input_area, footer_area] = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Min(2),
        ])
        .areas(area);

        let mut intro_lines: Vec<Line> = vec![
            Line::from(vec![
                "> ".into(),
                format!("Use your own {} API key", provider.display_name()).bold(),
            ]),
            "".into(),
            "  Paste or type your API key below. It will be stored locally in auth.json.".into(),
            "".into(),
        ];
        if state.prepopulated_from_env {
            intro_lines.push(
                format!(
                    "  Detected {} environment variable.",
                    provider.env_var_name()
                )
                .into(),
            );
            intro_lines.push(
                "  Paste a different key if you prefer to use another account."
                    .dim()
                    .into(),
            );
            intro_lines.push("".into());
        }
        Paragraph::new(intro_lines)
            .wrap(Wrap { trim: false })
            .render(intro_area, buf);

        let content_line: Line = if state.value.is_empty() {
            vec!["Paste or type your API key".dim()].into()
        } else {
            Line::from(state.value.clone())
        };
        Paragraph::new(content_line)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("API key")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(primary_accent_fg())),
            )
            .render(input_area, buf);

        let mut footer_lines: Vec<Line> = vec![
            Line::from(vec![
                "  Press ".dim(),
                self.confirm_binding().into(),
                " to save".dim(),
            ]),
            Line::from(vec![
                "  Press ".dim(),
                self.cancel_binding().into(),
                " to go back".dim(),
            ]),
        ];
        if let Some(error) = self.error_message() {
            footer_lines.push("".into());
            footer_lines.push(error.red().into());
        }
        Paragraph::new(footer_lines)
            .wrap(Wrap { trim: false })
            .render(footer_area, buf);
    }

    fn handle_api_key_entry_key_event(&mut self, key_event: &KeyEvent) -> bool {
        let mut should_save: Option<String> = None;
        let mut should_request_frame = false;

        {
            let mut guard = self.sign_in_state.write().unwrap();
            if let SignInState::ApiKeyEntry(state) = &mut *guard {
                if keys::CANCEL.is_pressed(*key_event) {
                    *guard = SignInState::PickByokProvider;
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::CONFIRM.is_pressed(*key_event) {
                    let trimmed = state.value.trim().to_string();
                    if trimmed.is_empty() {
                        self.set_error(Some("API key cannot be empty".to_string()));
                        should_request_frame = true;
                    } else {
                        should_save = Some(trimmed);
                    }
                } else {
                    match key_event.code {
                        KeyCode::Backspace => {
                            if state.prepopulated_from_env {
                                state.value.clear();
                                state.prepopulated_from_env = false;
                            } else {
                                state.value.pop();
                            }
                            self.set_error(/*message*/ None);
                            should_request_frame = true;
                        }
                        KeyCode::Char(c)
                            if key_event.kind == KeyEventKind::Press
                                && !key_event.modifiers.contains(KeyModifiers::SUPER)
                                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if state.prepopulated_from_env {
                                state.value.clear();
                                state.prepopulated_from_env = false;
                            }
                            state.value.push(c);
                            self.set_error(/*message*/ None);
                            should_request_frame = true;
                        }
                        _ => {}
                    }
                }
                // handled; let guard drop before potential save
            } else {
                return false;
            }
        }

        if let Some(api_key) = should_save {
            self.save_api_key(api_key);
        } else if should_request_frame {
            self.request_frame.schedule_frame();
        }
        true
    }

    fn handle_api_key_entry_paste(&mut self, pasted: String) -> bool {
        let trimmed = pasted.trim();
        if trimmed.is_empty() {
            return false;
        }

        let mut guard = self.sign_in_state.write().unwrap();
        if let SignInState::ApiKeyEntry(state) = &mut *guard {
            if state.prepopulated_from_env {
                state.value = trimmed.to_string();
                state.prepopulated_from_env = false;
            } else {
                state.value.push_str(trimmed);
            }
            self.set_error(/*message*/ None);
        } else {
            return false;
        }

        drop(guard);
        self.request_frame.schedule_frame();
        true
    }

    fn stored_key_for_provider(&self, provider: ByokProvider) -> Option<String> {
        read_stored_provider_api_key_from_disk(
            &self.forall_home,
            provider.env_var_name(),
            self.auth_credentials_store_mode,
            AuthKeyringBackendKind::default(),
        )
    }

    fn persist_provider_selection(&self, provider: ByokProvider) {
        let request_handle = self.app_server_request_handle.clone();
        let provider_id = provider.model_provider_id().to_string();
        tokio::spawn(async move {
            let _ = config_update::write_config_batch(
                request_handle,
                config_update::build_byok_provider_edits(&provider_id),
            )
            .await;
        });
    }

    fn confirm_byok_provider_selection(&mut self) {
        load_byok_env_files(&self.forall_home);
        self.set_error(/*message*/ None);
        let provider = self.highlighted_byok_provider;
        let env_key = provider.read_from_env().or_else(|| {
            forall_login::read_provider_key_from_env_files(
                &self.forall_home,
                Some(&self.workspace_cwd),
                provider.env_var_name(),
            )
        });
        let stored_key = self.stored_key_for_provider(provider);
        if env_key.is_some() || stored_key.is_some() {
            // Wait for the config write to land (it also reloads the app
            // server's model catalog) before completing this step, so the
            // model list reflects the provider that was just selected.
            let request_handle = self.app_server_request_handle.clone();
            let provider_id = provider.model_provider_id().to_string();
            let sign_in_state = self.sign_in_state.clone();
            let error = self.error.clone();
            let request_frame = self.request_frame.clone();
            tokio::spawn(async move {
                match config_update::write_config_batch(
                    request_handle,
                    config_update::build_byok_provider_edits(&provider_id),
                )
                .await
                {
                    Ok(_) => {
                        *sign_in_state.write().unwrap() = SignInState::ApiKeyConfigured;
                    }
                    Err(err) => {
                        *error.write().unwrap() =
                            Some(format!("Failed to save provider selection: {err}"));
                    }
                }
                request_frame.schedule_frame();
            });
            return;
        }
        self.persist_provider_selection(provider);
        if let Some(api_key) = env_key {
            self.save_api_key(api_key);
            return;
        }
        if let Some(api_key) = stored_key {
            self.save_api_key(api_key);
            return;
        }
        self.set_error(Some(format!(
            "Add {} to ~/.forall/.env or a .env file in your project, then press Enter to retry.",
            provider.env_var_name()
        )));
        self.request_frame.schedule_frame();
    }

    fn save_api_key(&mut self, api_key: String) {
        self.set_error(/*message*/ None);
        let request_handle = self.app_server_request_handle.clone();
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        let provider = self
            .sign_in_state
            .read()
            .ok()
            .and_then(|guard| match &*guard {
                SignInState::ApiKeyEntry(state) => Some(state.provider),
                _ => None,
            })
            .unwrap_or(self.highlighted_byok_provider);
        let model_provider = provider.model_provider_id().to_string();
        tokio::spawn(async move {
            match request_handle
                .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
                    request_id: onboarding_request_id(),
                    params: LoginAccountParams::ApiKey {
                        api_key: api_key.clone(),
                        model_provider: Some(model_provider),
                    },
                })
                .await
            {
                Ok(LoginAccountResponse::ApiKey {}) => {
                    *error.write().unwrap() = None;
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyConfigured;
                }
                Ok(LoginAccountResponse::Oauth { .. }) => {
                    *error.write().unwrap() =
                        Some("Unexpected response to the API key login request".to_string());
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyEntry(ApiKeyInputState {
                        value: api_key,
                        prepopulated_from_env: false,
                        provider,
                    });
                }
                Err(err) => {
                    *error.write().unwrap() = Some(format!("Failed to save API key: {err}"));
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyEntry(ApiKeyInputState {
                        value: api_key,
                        prepopulated_from_env: false,
                        provider,
                    });
                }
            }
            request_frame.schedule_frame();
        });
        self.request_frame.schedule_frame();
    }

    pub(crate) fn on_account_updated(&mut self, notification: AccountUpdatedNotification) {
        self.login_status = notification
            .auth_mode
            .map(LoginStatus::AuthMode)
            .unwrap_or(LoginStatus::NotAuthenticated);
    }

    /// Resolves an in-flight Forall OAuth login when the app server reports
    /// `account/login/completed`.
    pub(crate) fn on_login_completed(&mut self, notification: AccountLoginCompletedNotification) {
        let mut guard = self.sign_in_state.write().unwrap();
        let SignInState::ForallOauth(state) = &*guard else {
            return;
        };
        if let (Some(waiting_id), Some(completed_id)) =
            (state.login_id.as_ref(), notification.login_id.as_ref())
            && waiting_id != completed_id
        {
            return;
        }
        if notification.success {
            *guard = SignInState::ForallOauthComplete;
            drop(guard);
            self.set_error(/*message*/ None);
        } else {
            *guard = SignInState::PickAuthMode;
            drop(guard);
            self.set_error(Some(
                notification
                    .error
                    .unwrap_or_else(|| "Forall sign-in failed".to_string()),
            ));
        }
        self.request_frame.schedule_frame();
    }
}

impl StepStateProvider for AuthModeWidget {
    fn get_step_state(&self) -> StepState {
        let sign_in_state = self.sign_in_state.read().unwrap();
        match &*sign_in_state {
            SignInState::PickAuthMode
            | SignInState::ForallOauthStarting { .. }
            | SignInState::ForallOauth(_)
            | SignInState::PickByokProvider
            | SignInState::ApiKeyEntry(_) => StepState::InProgress,
            SignInState::ForallOauthComplete | SignInState::ApiKeyConfigured => StepState::Complete,
        }
    }
}

impl WidgetRef for AuthModeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let sign_in_state = self.sign_in_state.read().unwrap();
        match &*sign_in_state {
            SignInState::PickAuthMode => {
                self.render_auth_mode_picker(area, buf);
            }
            SignInState::ForallOauthStarting {
                cancel_requested: true,
                ..
            } => {
                // The user backed out while the start request was in flight;
                // show the picker again immediately.
                self.render_auth_mode_picker(area, buf);
            }
            SignInState::ForallOauthStarting {
                cancel_requested: false,
                ..
            } => {
                // No auth URL yet: renders the "Starting sign-in..." view.
                self.render_forall_oauth(area, buf, &ForallOauthState::default());
            }
            SignInState::ForallOauth(state) => {
                self.render_forall_oauth(area, buf, state);
            }
            SignInState::ForallOauthComplete => {
                self.render_forall_oauth_complete(area, buf);
            }
            SignInState::PickByokProvider => {
                self.render_byok_provider_picker(area, buf);
            }
            SignInState::ApiKeyEntry(state) => {
                self.render_api_key_entry(area, buf, state);
            }
            SignInState::ApiKeyConfigured => {
                self.render_api_key_configured(area, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use forall_app_server_client::AppServerRequestHandle;
    use forall_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
    use forall_app_server_client::InProcessAppServerClient;
    use forall_app_server_client::InProcessClientStartArgs;
    use forall_arg0::Arg0DispatchPaths;
    use forall_config::CloudConfigBundleLoader;
    use forall_config::types::AuthCredentialsStoreMode;

    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn widget_for_byok_onboarding() -> (AuthModeWidget, TempDir) {
        let forall_home = TempDir::new().unwrap();
        let forall_home_path = forall_home.path().to_path_buf();
        let config = ConfigBuilder::default()
            .forall_home(forall_home_path.clone())
            .build()
            .await
            .unwrap();
        let client = InProcessAppServerClient::start(InProcessClientStartArgs {
            arg0_paths: Arg0DispatchPaths::default(),
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            loader_overrides: Default::default(),
            strict_config: false,
            cloud_config_bundle: CloudConfigBundleLoader::default(),
            feedback: forall_feedback::ForallFeedback::new(),
            log_db: None,
            state_db: None,
            environment_manager: Arc::new(
                forall_app_server_client::EnvironmentManager::default_for_tests(),
            ),
            config_warnings: Vec::new(),
            session_source: serde_json::from_value(serde_json::json!("cli"))
                .expect("cli session source should deserialize"),
            enable_forall_api_key_env: false,
            client_name: "test".to_string(),
            client_version: "test".to_string(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
        })
        .await
        .unwrap();
        let mut widget = AuthModeWidget {
            request_frame: FrameRequester::test_dummy(),
            highlighted_auth_mode: AuthModeChoice::Forall,
            highlighted_byok_provider: ByokProvider::OpenAi,
            error: Arc::new(RwLock::new(None)),
            sign_in_state: Arc::new(RwLock::new(SignInState::PickAuthMode)),
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: AppServerRequestHandle::InProcess(client.request_handle()),
            animations_enabled: true,
            animations_suppressed: std::cell::Cell::new(false),
            forall_home: forall_home_path.clone(),
            workspace_cwd: forall_home_path,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            provider_env_key: Some("OPENROUTER_API_KEY".to_string()),
        };
        widget.highlighted_byok_provider = ByokProvider::OpenRouter;
        *widget.sign_in_state.write().unwrap() = SignInState::PickAuthMode;
        (widget, forall_home)
    }

    fn row_containing(buf: &Buffer, needle: &str) -> Option<u16> {
        (0..buf.area.height).find(|&y| {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            row.contains(needle)
        })
    }

    #[tokio::test]
    async fn onboarding_opens_auth_mode_picker() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;

        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickAuthMode
        ));
        assert_eq!(widget.highlighted_auth_mode, AuthModeChoice::Forall);
    }

    #[tokio::test]
    async fn auth_mode_picker_renders_forall_and_byok() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;
        let area = Rect::new(0, 0, 88, 16);
        let mut buf = Buffer::empty(area);
        widget.render_ref(area, &mut buf);

        assert_eq!(row_containing(&buf, "How do you want to sign in?"), Some(0));
        assert!(row_containing(&buf, "Forall account").is_some());
        assert!(row_containing(&buf, "Bring your own API key").is_some());
    }

    #[tokio::test]
    async fn byok_provider_picker_renders_all_five_providers() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;
        *widget.sign_in_state.write().unwrap() = SignInState::PickByokProvider;
        let area = Rect::new(0, 0, 72, 26);
        let mut buf = Buffer::empty(area);
        widget.render_ref(area, &mut buf);

        assert_eq!(row_containing(&buf, "Choose your model provider"), Some(0));
        assert_eq!(ByokProvider::ALL.len(), 5);
        assert!(row_containing(&buf, "OpenAI").is_some());
        assert!(row_containing(&buf, "OpenRouter").is_some());
        assert!(row_containing(&buf, "Azure OpenAI").is_some());
        assert!(row_containing(&buf, "4. Anthropic").is_some());
        assert!(row_containing(&buf, "5. Google Gemini").is_some());
        assert!(row_containing(&buf, "OPENAI_API_KEY").is_some());
        assert!(row_containing(&buf, "OPENROUTER_API_KEY").is_some());
        assert!(row_containing(&buf, "AZURE_OPENAI_API_KEY").is_some());
        assert!(row_containing(&buf, "ANTHROPIC_API_KEY").is_some());
        assert!(row_containing(&buf, "GEMINI_API_KEY").is_some());
    }

    #[tokio::test]
    async fn byok_provider_selection_persists_openrouter_auto_model() {
        let (_widget, _tmp) = widget_for_byok_onboarding().await;
        let edits = crate::config_update::build_byok_provider_edits("openrouter");
        let model_edit = edits
            .iter()
            .find(|edit| edit.key_path == "model")
            .expect("model edit should be present");
        assert_eq!(model_edit.value, serde_json::json!("openrouter/auto"));
    }

    #[tokio::test]
    async fn esc_before_login_start_response_cancels_late_login_id() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;
        *widget.sign_in_state.write().unwrap() = SignInState::ForallOauthStarting {
            attempt: 7,
            cancel_requested: false,
        };

        // Esc arrives before the `account/login/start` response.
        widget.cancel_active_attempt();
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::ForallOauthStarting {
                attempt: 7,
                cancel_requested: true,
            }
        ));

        // When the response finally delivers the login_id, the login must be
        // cancelled instead of entering the waiting state.
        let action = apply_login_start_response(
            &widget.sign_in_state,
            /*attempt*/ 7,
            "login-7".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        assert_eq!(action, LoginStartAction::CancelLogin("login-7".to_string()));
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickAuthMode
        ));
    }

    #[tokio::test]
    async fn login_start_response_then_esc_cancels_waiting_login() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;
        *widget.sign_in_state.write().unwrap() = SignInState::ForallOauthStarting {
            attempt: 3,
            cancel_requested: false,
        };

        // Inverse ordering: the response arrives first and is adopted.
        let action = apply_login_start_response(
            &widget.sign_in_state,
            /*attempt*/ 3,
            "login-3".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        assert_eq!(
            action,
            LoginStartAction::OpenBrowser("https://auth.example/authorize".to_string())
        );
        {
            let guard = widget.sign_in_state.read().unwrap();
            let SignInState::ForallOauth(state) = &*guard else {
                panic!("response should enter the waiting state");
            };
            assert_eq!(state.login_id.as_deref(), Some("login-3"));
        }

        // Esc afterwards takes the pre-existing waiting-state cancel path.
        widget.cancel_active_attempt();
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickAuthMode
        ));
    }

    #[tokio::test]
    async fn stale_login_start_response_is_cancelled_without_clobbering_new_attempt() {
        let (widget, _tmp) = widget_for_byok_onboarding().await;
        // A newer attempt (2) is pending when the response for the abandoned
        // attempt (1) arrives.
        *widget.sign_in_state.write().unwrap() = SignInState::ForallOauthStarting {
            attempt: 2,
            cancel_requested: false,
        };

        let action = apply_login_start_response(
            &widget.sign_in_state,
            /*attempt*/ 1,
            "login-1".to_string(),
            "https://auth.example/authorize".to_string(),
        );
        assert_eq!(action, LoginStartAction::CancelLogin("login-1".to_string()));
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::ForallOauthStarting {
                attempt: 2,
                cancel_requested: false,
            }
        ));
    }
}
