//! Sidebar navigation with user profile area at the top.
//!
//! Layout: top = avatar + username, middle = nav links, bottom = secure link status + lock.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::components::auth::{AuthCtx, AuthStatus};
use crate::i18n::keys;
use crate::i18n::t as tr;
use dioxus_bootstrap_css::theme::Theme;

/// Get the signal connection status indicator
fn get_signal_status_icon(status: &AuthStatus) -> (&'static str, &'static str) {
    match status {
        AuthStatus::LoggedIn { .. } => ("✓", "var(--bs-success)"),
        AuthStatus::LoggingIn | AuthStatus::Registering => ("⟳", "var(--bs-warning)"),
        AuthStatus::Disconnected => ("✗", "var(--bs-danger)"),
        AuthStatus::Reconnecting(_) => ("⟳", "var(--bs-warning)"),
        AuthStatus::Error(_) => ("⚠", "var(--bs-danger)"),
        AuthStatus::LoggedOut => ("○", "var(--bs-tertiary-color)"),
    }
}

// ─── Nav tab enum ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum NavTab {
    Home,
    Connect,
    Host,
}

// ─── Sidebar ─────────────────────────────────────────────────────────────────

#[component]
pub fn Sidebar(
    nav_tab: Signal<NavTab>,
    _theme: Signal<Theme>,
    locale: Signal<String>,
    show_lock_dialog: Signal<bool>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let AuthCtx { status, .. } = use_context::<AuthCtx>();

    let is_logged_in = matches!(status(), AuthStatus::LoggedIn { .. });
    let username = match status() {
        AuthStatus::LoggedIn { ref username } => username.clone(),
        _ => String::new(),
    };
    // Truncate for avatar display
    let display_name = if username.len() > 10 {
        format!("{}…", &username[..9])
    } else if username.is_empty() {
        tr(keys::SIDEBAR_NOT_LOGGED_IN, l).to_string()
    } else {
        username.clone()
    };
    // First letter for avatar
    let avatar_letter = username
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let avatar_bg = if is_logged_in {
        "var(--bs-success)"
    } else {
        "var(--bs-tertiary-color)"
    };
    
    // Signal connection status
    let (signal_icon, signal_color) = get_signal_status_icon(&status());
    let signal_text_owned: String;
    let signal_text: &str = match status() {
        AuthStatus::LoggedIn { .. } => "已连接",
        AuthStatus::LoggingIn => "登录中...",
        AuthStatus::Registering => "注册中...",
        AuthStatus::Disconnected => "已断开",
        AuthStatus::Reconnecting(attempt) => {
            signal_text_owned = format!("重连中 ({}次)", attempt);
            &signal_text_owned
        }
        AuthStatus::Error(_) => "错误",
        AuthStatus::LoggedOut => "未登录",
    };

    // Lock state: whether the UI is currently locked
    let is_locked = use_signal(|| false);

    rsx! {
        div {
            class: "d-flex flex-column flex-shrink-0 p-2",
            style: "width: 200px; background: var(--bs-tertiary-bg);
                    border-right: 1px solid var(--bs-border-color);",

            // ── Top: user avatar + name ────────────────────────────────
            div {
                class: "d-flex flex-column gap-2 p-2 mb-2 rounded-3",
                style: "background: rgba(var(--bs-primary-rgb), 0.08);
                       border: 1px solid rgba(var(--bs-primary-rgb), 0.15);",

                // User info row
                div { class: "d-flex align-items-center gap-2",
                    div {
                        class: "d-flex align-items-center justify-content-center rounded-circle flex-shrink-0",
                        style: "width: 36px; height: 36px;
                               background: {avatar_bg};
                               color: white; font-size: 16px; font-weight: 600;
                               letter-spacing: 0;",
                        "{avatar_letter}"
                    }
                    div {
                        class: "d-flex flex-column overflow-hidden",
                        span {
                            class: "fw-semibold text-truncate",
                            style: "font-size: 14px; line-height: 1.2;",
                            "{display_name}"
                        }
                        if is_logged_in {
                            span {
                                class: "small",
                                style: "color: var(--bs-success);",
                                "{tr(keys::SIDEBAR_LOGGED_IN_SHORT, l)}"
                            }
                        } else {
                            span {
                                class: "small",
                                style: "color: var(--bs-tertiary-color);",
                                "{tr(keys::SIDEBAR_NOT_LOGGED_IN_SHORT, l)}"
                            }
                        }
                    }
                }
                
                // Signal connection status indicator
                div { class: "d-flex align-items-center gap-1",
                    span {
                        style: "font-size: 12px; color: {signal_color}; font-weight: 600;",
                        "{signal_icon}"
                    }
                    span {
                        class: "small",
                        style: "color: {signal_color};",
                        "{signal_text}"
                    }
                }
            }

            // ── Middle: navigation links ──────────────────────────────
            Nav {
                pills: true,
                vertical: true,
                class: "flex-grow-1",

                NavItem {
                    NavLink {
                        active: nav_tab() == NavTab::Connect,
                        onclick: move |evt: Event<MouseData>| { evt.prevent_default(); nav_tab.set(NavTab::Connect); },
                        span { class: "me-2", "⇆" }
                        "{tr(keys::SIDEBAR_REMOTE_CTRL, l)}"
                    }
                }
                NavItem {
                    NavLink {
                        active: nav_tab() == NavTab::Home,
                        onclick: move |evt: Event<MouseData>| { evt.prevent_default(); nav_tab.set(NavTab::Home); },
                        span { class: "me-2", "🖥" }
                        "{tr(keys::SIDEBAR_DEVICES, l)}"
                    }
                }
                NavItem {
                    NavLink {
                        active: nav_tab() == NavTab::Host,
                        onclick: move |evt: Event<MouseData>| { evt.prevent_default(); nav_tab.set(NavTab::Host); },
                        span { class: "me-2", "◫" }
                        "{tr(keys::SIDEBAR_HOST_SERVICE, l)}"
                    }
                }
            }

            // ── Bottom: secure link status + lock icon ───────────────
            div {
                class: "d-flex align-items-center justify-content-between p-2 rounded-2",
                style: "background: rgba(var(--bs-success-rgb), 0.10);
                       border: 1px solid rgba(var(--bs-success-rgb), 0.22);",
                div {
                    class: "d-flex align-items-center gap-2",
                    // Green shield icon with checkmark
                    svg {
                        width: "16",
                        height: "16",
                        view_box: "0 0 16 16",
                        fill: "none",
                        style: "flex-shrink: 0;",
                        path {
                            d: "M8 1L2 4v4c0 3.5 2.6 6.8 6 7.5 3.4-.7 6-4 6-7.5V4L8 1z",
                            fill: "#3fb950",
                        }
                        path {
                            d: "M6.5 8L7.8 9.3 10 7",
                            stroke: "white",
                            stroke_width: "1.5",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            fill: "none",
                        }
                    }
                    span {
                        class: "small",
                        style: "color: var(--bs-success); font-weight: 500;",
                        "{tr(keys::SIDEBAR_SECURE_LINK, l)}"
                    }
                }
                // Lock icon
                div {
                    class: "d-flex align-items-center justify-content-center",
                    style: "cursor: pointer; opacity: 0.6; transition: opacity 0.2s;",
                    onclick: move |_| show_lock_dialog.set(true),
                    if is_locked() {
                        // Locked (solid lock)
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 16 16",
                            fill: "currentColor",
                            style: "color: var(--bs-warning);",
                            path {
                                d: "M8 1a3 3 0 0 0-3 3v2H4a1 1 0 0 0-1 1v7a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V7a1 1 0 0 0-1-1h-1V4a3 3 0 0 0-3-3zm2 5H6V4a2 2 0 1 1 4 0v2z",
                            }
                        }
                    } else {
                        // Unlocked (open lock)
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 16 16",
                            fill: "currentColor",
                            style: "color: var(--bs-secondary-color);",
                            path {
                                d: "M8 1a3 3 0 0 1 3 3v2H6V4a2 2 0 1 1 4 0h1a3 3 0 0 0-6 0v2H4a1 1 0 0 0-1 1v7a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V7a1 1 0 0 0-1-1h-1V4a3 3 0 0 0-3-3zm2 5H6V4a2 2 0 1 1 4 0v2z",
                            }
                        }
                    }
                }
            }

            // ── Lock dialog (Bootstrap Modal) ────────────────────────
            super::lock_ui::LockUIDialog { locale,  is_locked, show_lock_dialog }
        }
    }
}

