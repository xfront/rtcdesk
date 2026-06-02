//! Custom title bar: TitleBar, WinBtn.
//! Uses dioxus-bootstrap-css for theme toggle.

use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus::desktop::use_window;
use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
use dioxus_bootstrap_css::theme::Theme;

use crate::i18n::{t as tr};
use crate::i18n::keys;

const LOGO_BYTES: &[u8] = include_bytes!("../assets/favicon.webp");
pub static LOGO_DATA_URL: LazyLock<String> = LazyLock::new(|| {
    format!("data:image/webp;base64,{}", STANDARD.encode(LOGO_BYTES))
});

// ─── Title bar ──────────────────────────────────────────────────────────────

#[component]
pub fn TitleBar(title: String, theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let window = use_window();
    let (w1, w2, w3) = (window.clone(), window.clone(), window.clone());
    let l: &'static str = Box::leak(locale().into_boxed_str());

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
