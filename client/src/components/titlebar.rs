//! Custom title bar: TitleBar, WinBtn, ThemeToggleBtn.

use dioxus::desktop::use_window;
use dioxus::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::theme::{tc, Theme, DARK};

/// Theme info for dropdown items.
const THEME_ITEMS: &[(Theme, &str, &str)] = &[
    (Theme::Dark,       "🌙", "settings.theme_dark"),
    (Theme::SkyBlue,    "🔵", "settings.theme_sky_blue"),
    (Theme::Light,      "☀",  "settings.theme_light"),
    (Theme::GrassGreen, "🌿", "settings.theme_grass_green"),
    (Theme::Orange,     "🍊", "settings.theme_orange"),
    (Theme::Purple,     "🟣", "settings.theme_purple"),
];

// ─── Title bar ──────────────────────────────────────────────────────────────

#[component]
pub fn TitleBar(title: String, theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let window = use_window();
    let (w1, w2, w3) = (window.clone(), window.clone(), window.clone());
    let t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        div {
            class: "titlebar",
            onmousedown: move |_| {
                let win: &dioxus::desktop::tao::window::Window = &**window;
                let _ = win.drag_window();
            },

            // Logo + title
            div {
                class: "titlebar-left",
                div {
                    class: "titlebar-logo",
                    "R"
                }
                span { class: "text-pri", style: "font-size: 13px; font-weight: 600;", "{title}" }
                span { class: "text-dim", style: "font-size: 11px; margin-left: 2px;", "{tr(keys::APP_VERSION, l)}" }
            }

            // Window controls
            div {
                class: "titlebar-controls",
                onmousedown: move |evt| evt.stop_propagation(),

                ThemeToggleBtn { theme, locale }

                div { class: "titlebar-divider" }

                WinBtn {
                    icon: "⎯",
                    title: tr(keys::TITLE_MINIMIZE, l),
                    color: t.text_sec,
                    onclick: move |_| w1.set_minimized(true),
                }
                WinBtn {
                    icon: "⛶",
                    title: tr(keys::TITLE_MAXIMIZE, l),
                    color: t.text_sec,
                    onclick: move |_| {
                        let win: &dioxus::desktop::tao::window::Window = &**w2;
                        win.set_maximized(!win.is_maximized());
                    },
                }
                WinBtn {
                    icon: "✕",
                    title: tr(keys::TITLE_CLOSE, l),
                    color: DARK.danger,
                    onclick: move |_| w3.close(),
                }
            }
        }
    }
}

// ─── Theme dropdown ───────────────────────────────────────────────────────────

#[component]
pub fn ThemeToggleBtn(theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let mut open = use_signal(|| false);
    let mut hovered = use_signal(|| false);
    let t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());

    // Current theme icon
    let cur_icon = THEME_ITEMS.iter()
        .find(|(th, _, _)| *th == theme())
        .map(|(_, icon, _)| *icon)
        .unwrap_or("🌙");

    let is_dark_bg = matches!(theme(), Theme::Dark | Theme::SkyBlue);
    let bg = if hovered() || open() {
        if is_dark_bg { "rgba(255,255,255,0.1)" } else { "rgba(0,0,0,0.06)" }
    } else {
        "transparent"
    };

    let toggle_open = move |evt: dioxus::events::MouseEvent| {
        evt.stop_propagation();
        open.set(!open());
    };
    let close_menu = move |_| open.set(false);

    rsx! {
        div {
            class: "theme-dropdown-container",
            onmouseenter: move |_| hovered.set(true),
            onmouseleave: move |_| hovered.set(false),

            // Trigger
            div {
                class: "theme-toggle",
                style: "background: {bg};",
                title: tr(keys::SETTINGS_THEME, l),
                onclick: toggle_open,
                "{cur_icon}"
            }

            // Dropdown menu + backdrop
            if open() {
                // Transparent backdrop — clicking outside closes the menu
                div {
                    class: "theme-dropdown-backdrop",
                    onclick: close_menu,
                }
                {
                    let current_theme = theme();
                    let items: Vec<_> = THEME_ITEMS.iter().map(|(th, icon, key)| {
                        let is_active = *th == current_theme;
                        let item_bg = if is_active { "var(--accent-bg)" } else { "transparent" };
                        let th_c = *th;
                        let icon_c = *icon;
                        let key_c = *key;
                        rsx! {
                            div {
                                class: "theme-dropdown-item",
                                style: "background: {item_bg}; color: {t.text_pri};",
                                onclick: move |_| {
                                    theme.set(th_c);
                                    open.set(false);
                                    let mut cfg = load_config();
                                    cfg.theme = th_c;
                                    save_config(&cfg);
                                },
                                span { class: "theme-dropdown-icon", "{icon_c}" }
                                span { class: "theme-dropdown-label", "{tr(key_c, l)}" }
                            }
                        }
                    }).collect();

                    rsx! {
                        div {
                            class: "theme-dropdown-menu",
                            style: "border-color: {t.border}; background: {t.bg_card};",
                            onclick: move |evt| evt.stop_propagation(),
                            {items.into_iter()}
                        }
                    }
                }
            }
        }
    }
}

// ─── Generic window-chrome button ────────────────────────────────────────────

#[component]
pub fn WinBtn(
    icon: &'static str,
    title: &'static str,
    color: &'static str,
    onclick: EventHandler<()>,
) -> Element {
    let mut hovered = use_signal(|| false);
    let bg = if hovered() {
        "rgba(255,255,255,0.08)"
    } else {
        "transparent"
    };
    rsx! {
        div {
            class: "win-btn",
            style: "color: {color}; background: {bg};",
            title: title,
            onmouseenter: move |_| hovered.set(true),
            onmouseleave: move |_| hovered.set(false),
            onclick: move |_| onclick.call(()),
            "{icon}"
        }
    }
}
