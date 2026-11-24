//! Credential entry for /provider. The secret stays outside the chat composer and its history.
use crate::config_update;
use crate::legacy_core::config::Config;
use crate::onboarding::ByokProvider;
use crate::tui::Tui;
use crate::tui::TuiEvent;
use color_eyre::eyre::Result;
use color_eyre::eyre::eyre;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use forall_app_server_client::AppServerRequestHandle;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::LoginAccountParams;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::RequestId;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use tokio_stream::StreamExt;
use url::Url;

#[derive(Clone, Copy, PartialEq)]
enum Field {
    ApiKey,
    AzureEndpoint,
}

async fn read_field(
    tui: &mut Tui,
    provider: ByokProvider,
    field: Field,
    can_keep: bool,
    remote: bool,
) -> Result<Option<String>> {
    let mut events = tui.event_stream();
    let mut input = String::new();
    let mut error = "";
    let title = match field {
        Field::ApiKey => format!("{} API key", provider.display_name()),
        Field::AzureEndpoint => "Azure OpenAI endpoint".to_string(),
    };
    loop {
        tui.draw(7, |frame| {
            let hint = match field {
                Field::AzureEndpoint => "Enter your Azure resource URL, including /openai/v1.",
                Field::ApiKey if can_keep => {
                    "Paste a new key, or leave blank to keep the configured key."
                }
                Field::ApiKey => "Paste or type your API key.",
            };
            let value = if field == Field::ApiKey {
                "•".repeat(input.chars().count().min(60))
            } else {
                input.clone()
            };
            let mut lines = vec![
                Line::from(title.as_str().bold()),
                hint.into(),
                format!("> {value}").into(),
            ];
            if remote {
                lines.push("These settings are saved on the connected Forall server.".into());
            }
            lines.push("Enter to confirm   Esc to cancel".dim().into());
            lines.push(error.red().into());
            frame.render_widget_ref(
                Paragraph::new(lines).wrap(Wrap { trim: false }),
                frame.area(),
            );
        })?;
        match events.next().await {
            Some(TuiEvent::Paste(text)) => {
                // Allow a trailing newline from the clipboard but reject multiple entries.
                let text = text.trim();
                if text.chars().any(char::is_whitespace) || text.chars().any(char::is_control) {
                    error = "Paste a single value.";
                } else {
                    input.push_str(text);
                    error = "";
                }
            }
            Some(TuiEvent::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                    error = "";
                }
                KeyCode::Enter => {
                    if input.is_empty() && !can_keep {
                        error = "Enter a value to continue.";
                    } else if field == Field::AzureEndpoint
                        && !Url::parse(&input).is_ok_and(|url| {
                            url.scheme() == "https"
                                && url.has_host()
                                && url.username().is_empty()
                                && url.password().is_none()
                                && url.query().is_none()
                                && url.fragment().is_none()
                        })
                    {
                        error =
                            "Enter an HTTPS endpoint without credentials, a query, or a fragment.";
                    } else {
                        return Ok(Some(input));
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    error = "";
                }
                KeyCode::Char(ch)
                    if !ch.is_whitespace()
                        && !ch.is_control()
                        && !key.modifiers.intersects(
                            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                        ) =>
                {
                    input.push(ch);
                    error = "";
                }
                _ => {}
            },
            None => return Ok(None),
            _ => {}
        }
    }
}

pub(crate) async fn configure(
    tui: &mut Tui,
    config: &Config,
    server: AppServerRequestHandle,
    provider: ByokProvider,
    remote: bool,
) -> Result<bool> {
    let id = provider.model_provider_id();
    let can_keep = !remote
        && forall_login::provider_env_api_key_is_configured(
            provider.env_var_name(),
            &config.forall_home,
            config.cli_auth_credentials_store_mode,
            config.auth_keyring_backend_kind(),
        );
    let Some(key) = read_field(tui, provider, Field::ApiKey, can_keep, remote).await? else {
        return Ok(false);
    };
    if provider == ByokProvider::Azure
        && (remote
            || config
                .model_providers
                .get(id)
                .and_then(|info| info.base_url.as_ref())
                .is_none())
    {
        let Some(url) = read_field(tui, provider, Field::AzureEndpoint, false, remote).await?
        else {
            return Ok(false);
        };
        config_update::write_config_batch(
            server.clone(),
            vec![
                config_update::replace_config_value(
                    "model_providers.azure.name",
                    provider.display_name().into(),
                ),
                config_update::replace_config_value("model_providers.azure.base_url", url.into()),
            ],
        )
        .await?;
    }
    if key.is_empty() {
        let mut edits = config_update::build_byok_provider_edits(id);
        if config.model_provider_id == id {
            edits.retain(|edit| edit.key_path != "model");
        }
        config_update::write_config_batch(server, edits).await?;
    } else {
        let response: LoginAccountResponse = server
            .request_typed(ClientRequest::LoginAccount {
                request_id: RequestId::String(uuid::Uuid::new_v4().to_string()),
                params: LoginAccountParams::ApiKey {
                    api_key: key,
                    model_provider: Some(id.into()),
                },
            })
            .await?;
        if !matches!(response, LoginAccountResponse::ApiKey {}) {
            return Err(eyre!("Unexpected response while saving the provider key"));
        }
    }
    Ok(true)
}
