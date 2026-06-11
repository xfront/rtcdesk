//! Login/Register component with auto-login on startup.
//!
//! Provides login and registration forms, communicating with host-service via IPC.
//! When saved credentials exist in config, auto-login is triggered on app startup.
//! All signal server communication is proxied through host-service.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use dioxus_bootstrap_css::theme::Theme;
use super::SettingsCard;
use crate::rpc_client::RpcClient;

// ─── Auth status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AuthStatus {
    LoggedOut,
    LoggingIn,
    Registering,
    LoggedIn { username: String },
    Error(String),
    /// Signal server disconnected, attempting to reconnect
    Disconnected,
    /// Reconnecting to signal server (includes attempt number)
    Reconnecting(u32),
}

#[derive(Clone)]
pub struct AuthCtx {
    pub status: Signal<AuthStatus>,
    pub username: Signal<String>,
    pub password: Signal<String>,
    pub rpc_client: std::sync::Arc<std::sync::Mutex<RpcClient>>,
}

impl AuthCtx {
    pub fn new(rpc_client: std::sync::Arc<std::sync::Mutex<RpcClient>>) -> Self {
        let cfg = load_config();
        Self {
            status: Signal::new(AuthStatus::LoggedOut),
            username: Signal::new(cfg.login_username),
            password: Signal::new(cfg.login_password),
            rpc_client,
        }
    }

    /// Trigger auto-login if saved credentials exist.
    /// Called once on app startup via `use_effect`.
    pub fn try_auto_login(&self, locale: String) {
        log::info!("[auto-login] try_auto_login called");
        let cfg = load_config();
        let uname = cfg.login_username;
        let pwd = cfg.login_password;
        log::info!("[auto-login] username='{}' pwd_len={}", uname, pwd.len());
        if uname.is_empty() || pwd.is_empty() {
            log::info!("[auto-login] no saved credentials, skipping");
            return; // no saved credentials
        }
        let mut status = self.status;
        let rpc_client = self.rpc_client.clone();
        spawn(async move {
            status.set(AuthStatus::LoggingIn);
            let l: &'static str = Box::leak(locale.into_boxed_str());
            let result = rpc_client.lock().unwrap().user_login(uname.clone(), pwd.clone()).await;
            match result {
                Ok(_) => {
                    log::info!("Auto-login ok");
                    status.set(AuthStatus::LoggedIn { username: uname });
                }
                Err(e) => {
                    log::warn!("Auto-login failed: {}", e);
                    // Don't show error on auto-login failure, just stay LoggedOut
                    status.set(AuthStatus::LoggedOut);
                }
            }
        });
    }
}

// ─── Auth section (for Settings tab) ─────────────────────────────────────

#[component]
pub fn AuthSection(
    theme: Signal<Theme>,
    locale: Signal<String>,
) -> Element {
    let AuthCtx { status, mut username, mut password, rpc_client } = use_context::<AuthCtx>();
    let _t = theme();
    let l: &'static str = Box::leak(locale().into_boxed_str());

    let current_status = status();
    let status_text_owned: String;
    let (status_text, status_badge_color) = match &current_status {
        AuthStatus::LoggedOut => (tr(keys::AUTH_NOT_LOGGED_IN, l), Color::Secondary),
        AuthStatus::LoggingIn => (tr(keys::AUTH_LOGGING_IN, l), Color::Warning),
        AuthStatus::Registering => (tr(keys::AUTH_REGISTERING, l), Color::Warning),
        AuthStatus::LoggedIn { username } => (username.as_str(), Color::Success),
        AuthStatus::Error(msg) => (msg.as_str(), Color::Danger),
        AuthStatus::Disconnected => {
            status_text_owned = "信号服务器已断开".to_string();
            (status_text_owned.as_str(), Color::Danger)
        }
        AuthStatus::Reconnecting(attempt) => {
            status_text_owned = format!("重连中... (第{}次)", attempt);
            (status_text_owned.as_str(), Color::Warning)
        }
    };

    let is_busy = matches!(&current_status, AuthStatus::LoggingIn | AuthStatus::Registering);
    let is_logged_in = matches!(&current_status, AuthStatus::LoggedIn { .. });

    let login_label = tr(keys::AUTH_BTN_LOGIN, l);
    let login_busy_label = tr(keys::AUTH_LOGGING_IN, l);
    let register_label = tr(keys::AUTH_BTN_REGISTER, l);
    let register_busy_label = tr(keys::AUTH_REGISTERING, l);
    let logout_label = tr(keys::AUTH_BTN_LOGOUT, l);

    rsx! {
        SettingsCard {
            title: tr(keys::AUTH_ACCOUNT, l),
            children: rsx! {
                // Status display
                div {
                    class: "d-flex align-items-center justify-content-between mb-3
                            p-2 rounded-2",
                    style: "background: var(--bs-tertiary-bg);",
                    div { class: "d-flex align-items-center gap-2",
                        Badge { color: status_badge_color, class: "me-1", "{status_text}" }
                    }
                    if is_logged_in {
                        Button {
                            color: Color::Danger,
                            outline: true,
                            size: Size::Sm,
                            onclick: {
                                let mut s = status;
                                let mut u = username;
                                let mut p = password;
                                move |_| {
                                    s.set(AuthStatus::LoggedOut);
                                    u.set(String::new());
                                    p.set(String::new());
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

                super::connect::FormField { label: tr(keys::AUTH_LABEL_USER, l),
                    Input {
                        r#type: "text",
                        placeholder: tr(keys::AUTH_PLACEHOLDER_USER, l),
                        value: "{username}",
                        disabled: is_busy,
                        oninput: move |e: FormEvent| username.set(e.value()),
                    }
                }

                super::connect::FormField { label: tr(keys::AUTH_LABEL_PW, l),
                    Input {
                        r#type: "password",
                        placeholder: tr(keys::AUTH_PLACEHOLDER_PW, l),
                        value: "{password}",
                        disabled: is_busy,
                        oninput: move |e: FormEvent| password.set(e.value()),
                    }
                }

                div { class: "d-flex gap-2 mt-2",
                    Button {
                        color: Color::Primary,
                        class: "flex-grow-1",
                        disabled: is_busy || username().is_empty() || password().is_empty(),
                        onclick: {
                            let rpc_client = rpc_client.clone();
                            move |_| do_login(username(), password(), status, l.to_string(), rpc_client.clone())
                        },
                        if matches!(status(), AuthStatus::LoggingIn) {
                            Spinner { style: SpinnerStyle::Border, size: Size::Sm, class: "me-1" }
                            "{login_busy_label}"
                        } else {
                            "{login_label}"
                        }
                    }

                    Button {
                        color: Color::Success,
                        outline: true,
                        class: "flex-grow-1",
                        disabled: is_busy || username().is_empty() || password().is_empty(),
                        onclick: {
                            let rpc_client = rpc_client.clone();
                            move |_| do_register(username(), password(), status, l.to_string(), rpc_client.clone())
                        },
                        if matches!(status(), AuthStatus::Registering) {
                            Spinner { style: SpinnerStyle::Border, size: Size::Sm, class: "me-1" }
                            "{register_busy_label}"
                        } else {
                            "{register_label}"
                        }
                    }
                }

                if let AuthStatus::Error(msg) = &current_status {
                    Alert {
                        color: Color::Danger,
                        class: "mt-2 mb-0 py-2",
                        "{msg}"
                    }
                }

                p {
                    class: "text-secondary small mt-2 mb-0 lh-sm",
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
    rpc_client: std::sync::Arc<std::sync::Mutex<RpcClient>>,
) {
    spawn(async move {
        status_signal.set(AuthStatus::LoggingIn);

        let result = rpc_client.lock().unwrap().user_login(username.clone(), password.clone()).await;

        match result {
            Ok(_token) => {
                let mut cfg = load_config();
                cfg.login_username = username.clone();
                cfg.login_password = password.clone();
                save_config(&cfg);

                status_signal.set(AuthStatus::LoggedIn { username });
            }
            Err(e) => {
                status_signal.set(AuthStatus::Error(e.to_string()));
            }
        }
    });
}

fn do_register(
    username: String,
    password: String,
    mut status_signal: Signal<AuthStatus>,
    l: String,
    rpc_client: std::sync::Arc<std::sync::Mutex<RpcClient>>,
) {
    spawn(async move {
        status_signal.set(AuthStatus::Registering);

        let result = rpc_client.lock().unwrap().user_register(username.clone(), password.clone(), String::new()).await;

        match result {
            Ok(_token) => {
                let mut cfg = load_config();
                cfg.login_username = username.clone();
                cfg.login_password = password.clone();
                save_config(&cfg);

                status_signal.set(AuthStatus::LoggedIn { username });
            }
            Err(e) => {
                status_signal.set(AuthStatus::Error(e.to_string()));
            }
        }
    });
}


