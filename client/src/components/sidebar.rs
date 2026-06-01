//! Sidebar navigation component.

use dioxus::prelude::*;

use crate::components::auth::{AuthCtx, AuthStatus};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use crate::theme::{tc, Theme};

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
pub fn Sidebar(nav_tab: Signal<NavTab>, theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let AuthCtx { status, .. } = use_context::<AuthCtx>();

    let (status_color, status_text) = match status() {
        AuthStatus::LoggedIn { ref username } => {
            (t.accent, tf(keys::SIDEBAR_LOGGED_IN, l, &[("username", username)]))
        }
        _ => (t.text_dim, tr(keys::SIDEBAR_NOT_LOGGED_IN, l).to_string()),
    };
    // Truncate username display if too long
    let status_text = if status_text.len() > 18 {
        format!("{}…", &status_text[..17])
    } else {
        status_text
    };

    rsx! {
        div {
            class: "sidebar",
            NavItem {
                icon: "⇆", label: tr(keys::SIDEBAR_REMOTE_CTRL, l),
                active: nav_tab() == NavTab::Connect,
                onclick: move |_| nav_tab.set(NavTab::Connect),
                theme,
            }
            NavItem {
                icon: "🖥", label: tr(keys::SIDEBAR_DEVICES, l),
                active: nav_tab() == NavTab::Home,
                onclick: move |_| nav_tab.set(NavTab::Home),
                theme,
            }
            NavItem {
                icon: "◫", label: tr(keys::SIDEBAR_HOST_SERVICE, l),
                active: nav_tab() == NavTab::Host,
                onclick: move |_| nav_tab.set(NavTab::Host),
                theme,
            }
            NavItem {
                icon: "⚙", label: tr(keys::SIDEBAR_SETTINGS, l),
                active: nav_tab() == NavTab::Settings,
                onclick: move |_| nav_tab.set(NavTab::Settings),
                theme,
            }

            div { class: "flex-1" }

            // Status indicator
            div {
                class: "sidebar-status",
                div {
                    class: "sidebar-status-dot",
                    style: "background: {status_color};",
                }
                span { class: "text-sec", style: "font-size: 11px;", "{status_text}" }
            }
        }
    }
}

// ─── Navigation item ─────────────────────────────────────────────────────────

#[component]
pub fn NavItem(
    icon: &'static str,
    label: &'static str,
    active: bool,
    onclick: EventHandler<()>,
    theme: Signal<Theme>,
) -> Element {
    let mut hovered = use_signal(|| false);
    let t = tc(theme());

    let (bg, text_clr, accent_bar) = if active {
        (String::from("var(--accent-bg)"), t.text_pri, t.accent)
    } else if hovered() {
        (String::from("var(--hover-overlay)"), t.text_pri, "transparent")
    } else {
        (String::from("transparent"), t.text_sec, "transparent")
    };

    let active_weight = if active { "600" } else { "400" };

    rsx! {
        div {
            class: "nav-item",
            style: "background: {bg};",
            onmouseenter: move |_| hovered.set(true),
            onmouseleave: move |_| hovered.set(false),
            onclick: move |_| onclick.call(()),

            div {
                class: "nav-item-accent-bar",
                style: "background: {accent_bar};",
            }
            span { class: "nav-item-icon", "{icon}" }
            span { style: "font-size: 13px; color: {text_clr}; font-weight: {active_weight};", "{label}" }
        }
    }
}
