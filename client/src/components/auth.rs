//! Login/Register component.
//!
//! Provides login and registration forms, communicating with signaling server via WebSocket.

use dioxus::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use crate::theme::{tc, Theme};

// ─── Auth status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AuthStatus {
    LoggedOut,
    LoggingIn,
    Registering,
    LoggedIn { username: String },
    Error(String),
}

#[derive(Clone)]
pub struct AuthCtx {
    pub status: Signal<AuthStatus>,
    pub username: Signal<String>,
    pub password: Signal<String>,
}

impl AuthCtx {
    pub fn new() -> Self {
        let cfg = load_config();
        Self {
            status: Signal::new(AuthStatus::LoggedOut),
            username: Signal::new(cfg.login_username),
            password: Signal::new(cfg.login_password),
        }
    }
}

// ─── Auth section (for Settings tab) ─────────────────────────────────────

#[component]
pub fn AuthSection(
    theme: Signal<Theme>,
    locale: Signal<String>,
) -> Element {
    let AuthCtx { mut status, mut username, mut password } = use_context::<AuthCtx>();
    let t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());

    let current_status = status();
    let status_display = match &current_status {
        AuthStatus::LoggedOut => (tr(keys::AUTH_NOT_LOGGED_IN, l), t.text_dim, "○"),
        AuthStatus::LoggingIn => (tr(keys::AUTH_LOGGING_IN, l), t.warn, "⏳"),
        AuthStatus::Registering => (tr(keys::AUTH_REGISTERING, l), t.warn, "⏳"),
        AuthStatus::LoggedIn { username } => (username.as_str(), t.accent, "●"),
        AuthStatus::Error(msg) => (msg.as_str(), t.danger, "⚠"),
    };

    let is_busy = matches!(&current_status, AuthStatus::LoggingIn | AuthStatus::Registering);
    let is_logged_in = matches!(&current_status, AuthStatus::LoggedIn { .. });

    let login_label = tr(keys::AUTH_BTN_LOGIN, l);
    let login_busy_label = tr(keys::AUTH_LOGGING_IN, l);
    let register_label = tr(keys::AUTH_BTN_REGISTER, l);
    let register_busy_label = tr(keys::AUTH_REGISTERING, l);
    let logout_label = tr(keys::AUTH_BTN_LOGOUT, l);

    rsx! {
        super::settings::SettingsSection { title: tr(keys::AUTH_ACCOUNT, l), theme,
            children: rsx! {
                // Status display
                div {
                    class: "flex-between",
                    style: "margin-bottom: 14px; padding: 8px 12px; border-radius: 8px; background: var(--bg-ter);",
                    div {
                        class: "flex-center-row",
                        style: "gap: 8px;",
                        span { style: "font-size: 14px;", "{status_display.2}" }
                        span {
                            style: "font-size: 14px; font-weight: 600; color: {status_display.1};",
                            "{status_display.0}"
                        }
                    }
                    if is_logged_in {
                        button {
                            class: "btn-outline-danger",
                            style: "font-size: 12px; padding: 4px 12px;",
                            onclick: {
                                let mut s = status;
                                move |_| {
                                    s.set(AuthStatus::LoggedOut);
                                    let mut cfg = load_config();
                                    cfg.login_username.clear();
                                    cfg.login_password.clear();
                                    save_config(&cfg);
                                }
                            },
                            "{logout_label}"
                        }
                    }
                }

                super::connect::FormField { label: tr(keys::AUTH_LABEL_USER, l), theme,
                    input {
                        r#type: "text",
                        class: "input",
                        placeholder: tr(keys::AUTH_PLACEHOLDER_USER, l),
                        value: "{username}",
                        disabled: is_busy,
                        oninput: move |e| username.set(e.value()),
                    }
                }

                super::connect::FormField { label: tr(keys::AUTH_LABEL_PW, l), theme,
                    input {
                        r#type: "password",
                        class: "input",
                        placeholder: tr(keys::AUTH_PLACEHOLDER_PW, l),
                        value: "{password}",
                        disabled: is_busy,
                        oninput: move |e| password.set(e.value()),
                    }
                }

                div {
                    class: "flex-center-row",
                    style: "gap: 8px; margin-top: 4px;",

                    button {
                        class: "btn-primary-sm",
                        style: "flex: 1;",
                        disabled: is_busy || username().is_empty() || password().is_empty(),
                        onclick: move |_| do_login(username(), password(), status, l.to_string()),
                        if matches!(status(), AuthStatus::LoggingIn) { "{login_busy_label}" } else { "{login_label}" }
                    }

                    button {
                        class: "btn-outline-accent",
                        style: "flex: 1;",
                        disabled: is_busy || username().is_empty() || password().is_empty(),
                        onclick: move |_| do_register(username(), password(), status, l.to_string()),
                        if matches!(status(), AuthStatus::Registering) { "{register_busy_label}" } else { "{register_label}" }
                    }
                }

                if let AuthStatus::Error(msg) = &current_status {
                    div {
                        class: "text-danger",
                        style: "font-size: 12px; margin-top: 8px; line-height: 1.5;",
                        "{msg}"
                    }
                }

                div {
                    class: "text-dim text-11",
                    style: "margin-top: 10px; line-height: 1.5;",
                    "{tr(keys::AUTH_HELP, l)}"
                }
            }
        }
    }
}

fn do_login(
    username: String,
    password: String,
    mut status_signal: Signal<AuthStatus>,
    l: String,
) {
    spawn(async move {
        status_signal.set(AuthStatus::LoggingIn);

        let result = ws_auth_request(&username, &password, "login", &l).await;

        match result {
            Ok(_token) => {
                let mut cfg = load_config();
                cfg.login_username = username.clone();
                cfg.login_password = password.clone();
                save_config(&cfg);

                status_signal.set(AuthStatus::LoggedIn { username });
            }
            Err(e) => {
                status_signal.set(AuthStatus::Error(e));
            }
        }
    });
}

fn do_register(
    username: String,
    password: String,
    mut status_signal: Signal<AuthStatus>,
    l: String,
) {
    spawn(async move {
        status_signal.set(AuthStatus::Registering);

        let result = ws_auth_request(&username, &password, "register", &l).await;

        match result {
            Ok(_token) => {
                let mut cfg = load_config();
                cfg.login_username = username.clone();
                cfg.login_password = password.clone();
                save_config(&cfg);

                status_signal.set(AuthStatus::LoggedIn { username });
            }
            Err(e) => {
                status_signal.set(AuthStatus::Error(e));
            }
        }
    });
}

async fn ws_auth_request(username: &str, password: &str, action: &str, l: &str) -> Result<String, String> {
    use futures_util::{SinkExt, StreamExt};
    use shared::signal::SignalMessage;

    let cfg = load_config();
    let server_addr = cfg.server_addr;
    if server_addr.is_empty() {
        return Err(tr(keys::AUTH_NO_SERVER, l).to_string());
    }

    let ws_url = format!("ws://{}/ws", server_addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| tf(keys::AUTH_CONN_FAILED, l, &[("e", &e.to_string())]))?;

    let (mut ws_write, mut ws_read) = ws_stream.split();

    let msg = match action {
        "login" => SignalMessage::Login {
            username: username.to_string(),
            password: password.to_string(),
        },
        "register" => SignalMessage::UserRegister {
            username: username.to_string(),
            password: password.to_string(),
        },
        _ => return Err(tr(keys::AUTH_UNKNOWN_OP, l).to_string()),
    };

    let json = serde_json::to_string(&msg)
        .map_err(|e| tf(keys::AUTH_SERIALIZE_FAILED, l, &[("e", &e.to_string())]))?;
    ws_write
        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
        .await
        .map_err(|e| tf(keys::AUTH_SEND_FAILED, l, &[("e", &e.to_string())]))?;

    let response = tokio::time::timeout(std::time::Duration::from_secs(10), ws_read.next())
        .await
        .map_err(|_| tr(keys::AUTH_TIMEOUT, l).to_string())?;

    let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) = response else {
        return Err(tr(keys::AUTH_DISCONNECTED, l).to_string());
    };

    let resp: SignalMessage =
        serde_json::from_str(&text)
            .map_err(|e| tf(keys::AUTH_PARSE_FAILED, l, &[("e", &e.to_string())]))?;

    let unexpected = tr(keys::AUTH_UNEXPECTED_RESP, l).to_string();

    match action {
        "login" => match resp {
            SignalMessage::LoginResult { success, token, reason } => {
                if success { Ok(token) } else { Err(reason) }
            }
            _ => Err(unexpected.clone()),
        },
        "register" => match resp {
            SignalMessage::UserRegisterResult { success, message } => {
                if success { Ok(String::new()) } else { Err(message) }
            }
            _ => Err(unexpected.clone()),
        },
        _ => Err(unexpected),
    }
}
