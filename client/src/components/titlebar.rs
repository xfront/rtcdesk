//! Custom title bar with dropdown menu.
//! Uses dioxus-bootstrap-css for theme toggle and Dropdown component.

use dioxus::desktop::use_window;
use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
use dioxus_bootstrap_css::theme::Theme;

use crate::components::auth::{AuthCtx, AuthStatus};
use crate::config::{load_config, save_config};
use crate::i18n::keys;
use crate::i18n::t as tr;

use crate::components::icon::LOGO_DATA_URL;

#[component]
pub fn TitleBar(title: String, theme: Signal<Theme>, locale: Signal<String>, show_settings_dialog: Signal<bool>, show_lock_dialog: Signal<bool>) -> Element {
    let window = use_window();
    let (w1, w2, w3, w4) = (window.clone(), window.clone(), window.clone(), window.clone());
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let AuthCtx { mut status, mut username, mut password, .. } = use_context::<AuthCtx>();
    let is_logged_in = matches!(status(), AuthStatus::LoggedIn { .. });
    let mut show_menu = use_signal(|| false);
    let is_locked = use_signal(|| false);

    rsx! {
        div {
            class: "d-flex align-items-center flex-shrink-0 px-3",
            style: "height: 40px; background: var(--bs-tertiary-bg);
                    border-bottom: 1px solid var(--bs-border-color);
                    -webkit-app-region: drag;",
            onmousedown: move |_| {
                let win: &dioxus::desktop::tao::window::Window = &**window;
                let _ = win.drag_window();
            },

            // Logo + title
            div {
                class: "d-flex align-items-center gap-2 flex-grow-1 pe-none",
                img {
                    src: "{LOGO_DATA_URL.as_str()}",
                    style: "width: 22px; height: 22px; flex-shrink: 0;",
                }
                span { class: "small fw-semibold", style: "color: var(--bs-body-color);", "{title}" }
                span { class: "small ms-1", style: "color: var(--bs-tertiary-color);", "{tr(keys::APP_VERSION, l)}" }
            }

            // Window controls
            div {
                class: "d-flex align-items-center gap-2",
                style: "pointer-events: all; -webkit-app-region: no-drag;",
                onmousedown: move |evt| evt.stop_propagation(),

                ThemeToggle { theme }

                div { style: "width: 1px; height: 16px; background: var(--bs-border-color); margin: 0 2px;" }

                // ── Dropdown menu ────────────────────────────
                Dropdown {
                    toggle_class: "border-0",
                    open: show_menu,
                    toggle: rsx! {
                        Button {
                            color: Color::Secondary,
                            outline: true,
                            size: Size::Sm,
                            class: "border-0",
                            "☰"
                        }
                    },
                    menu: rsx! {
                        DropdownItem {
                            onclick: move |_| {
                                show_settings_dialog.set(true);
                                show_menu.set(false);
                            },
                            "⚙ {tr(keys::MENU_SETTINGS, l)}"
                        }
                        DropdownItem {
                            onclick: move |_| {
                                show_lock_dialog.set(true);
                                show_menu.set(false);
                            },
                            "{tr(keys::MENU_LOCK_UI, l)}"
                        }
                        // Separator
                        div {
                            class: "dropdown-divider",
                        }
                        if is_logged_in {
                            DropdownItem {
                                onclick: move |_| {
                                    status.set(AuthStatus::LoggedOut);
                                    username.set(String::new());
                                    password.set(String::new());
                                    let mut cfg = load_config();
                                    cfg.login_username.clear();
                                    cfg.login_password.clear();
                                    save_config(&cfg);
                                    show_menu.set(false);
                                },
                                "🔒 {tr(keys::MENU_LOGOUT, l)}"
                            }
                        }
                        DropdownItem {
                            onclick: move |_| {
                                w4.close();
                            },
                            style: "color: var(--bs-danger);",
                            "✕ {tr(keys::MENU_EXIT, l)}"
                        }
                    },
                }

                WinBtn {
                    icon: "⎯",
                    title: tr(keys::TITLE_MINIMIZE, l),
                    onclick: move |_| w1.set_minimized(true),
                }
                WinBtn {
                    icon: "⛶",
                    title: tr(keys::TITLE_MAXIMIZE, l),
                    onclick: move |_| {
                        let win: &dioxus::desktop::tao::window::Window = &**w2;
                        win.set_maximized(!win.is_maximized());
                    },
                }
                WinBtn {
                    icon: "✕",
                    title: tr(keys::TITLE_CLOSE, l),
                    danger: true,
                    onclick: move |_| w3.close(),
                }
            }
        }
    }
}

// ─── Generic window-chrome button ───────────────────────────────────────────

#[component]
pub fn WinBtn(
    icon: &'static str,
    title: &'static str,
    #[props(default = false)] danger: bool,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        Button {
            color: if danger { Color::Danger } else { Color::Secondary },
            outline: true,
            size: Size::Sm,
            class: "border-0",
            title,
            onclick: move |_| onclick.call(()),
            "{icon}"
        }
    }
}
