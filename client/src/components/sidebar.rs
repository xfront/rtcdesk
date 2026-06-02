//! Sidebar navigation using Bootstrap Nav component.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::components::auth::{AuthCtx, AuthStatus};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use dioxus_bootstrap_css::theme::Theme;

// ─── Nav tab enum ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum NavTab {
    Home,
    Connect,
    Host,
    Settings,
}

// ─── Sidebar ─────────────────────────────────────────────────────────────────

#[component]
pub fn Sidebar(nav_tab: Signal<NavTab>, _theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let AuthCtx { status, .. } = use_context::<AuthCtx>();

    let (status_color, status_text) = match status() {
        AuthStatus::LoggedIn { ref username } => {
            ("var(--bs-success)", tf(keys::SIDEBAR_LOGGED_IN, l, &[("username", username)]))
        }
        _ => ("var(--bs-tertiary-color)", tr(keys::SIDEBAR_NOT_LOGGED_IN, l).to_string()),
    };
    // Truncate username display if too long
    let status_text = if status_text.len() > 18 {
        format!("{}…", &status_text[..17])
    } else {
        status_text
    };

    rsx! {
        div {
            class: "d-flex flex-column flex-shrink-0 gap-2 p-2",
            style: "width: 200px; background: var(--bs-tertiary-bg);
                    border-right: 1px solid var(--bs-border-color);",

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
                NavItem {
                    NavLink {
                        active: nav_tab() == NavTab::Settings,
                        onclick: move |evt: Event<MouseData>| { evt.prevent_default(); nav_tab.set(NavTab::Settings); },
                        span { class: "me-2", "⚙" }
                        "{tr(keys::SIDEBAR_SETTINGS, l)}"
                    }
                }
            }

            // Status indicator
            div {
                class: "d-flex align-items-center gap-2 p-2 rounded-2 mt-auto",
                style: "background: rgba(var(--bs-success-rgb), 0.12); border: 1px solid rgba(var(--bs-success-rgb), 0.25);",
                div {
                    class: "rounded-circle flex-shrink-0",
                    style: "width: 7px; height: 7px; background: {status_color}; box-shadow: 0 0 6px {status_color};",
                }
                span {
                    class: "small",
                    style: "color: var(--bs-secondary-color);",
                    "{status_text}"
                }
            }
        }
    }
}
