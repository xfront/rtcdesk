//! Connect tab: local device ID display, remote ID/password input, connect action.

use dioxus::prelude::*;

use crate::clipboard::set_system_clipboard;
use crate::config::{load_config, save_config, HistoryEntry};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use crate::theme::{tc, Theme};
use crate::Page;

// ─── Connect tab ─────────────────────────────────────────────────────────────

#[component]
pub fn ConnectTab(
    page: Signal<Page>,
    server_addr: Signal<String>,
    remote_id: Signal<String>,
    quality: Signal<usize>,
    theme: Signal<Theme>,
    locale: Signal<String>,
    password: Signal<String>,
    local_id: Signal<String>,
) -> Element {
    let _t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let mut conn_mode = use_signal(|| "desktop".to_string());
    let mut host_password = use_signal(move || load_config().host_password.clone());

    let addr = server_addr();
    let blocked = remote_id().is_empty();

    rsx! {
        div { class: "connect-page",

            // ── Section: Allow control of this device ──
            div { class: "connect-section",
                h2 { class: "connect-section-title", "{tr(keys::CONNECT_TITLE_LOCAL, l)}" }

                div { class: "connect-id-grid",
                    // Left: Device ID
                    div { class: "connect-id-block",
                        span { class: "connect-field-label", "{tr(keys::CONNECT_LABEL_LOCAL_ID, l)}" }
                        div { class: "connect-id-row",
                            span { class: "connect-id-value", "{format_local_id(&local_id())}" }
                            button {
                                class: "connect-copy-btn",
                                onclick: move |_| {
                                    let id = format_local_id(&local_id());
                                    let _ = set_system_clipboard(&id);
                                },
                                "\u{1F4CB}"
                            }
                        }
                    }

                    // Divider
                    div { class: "connect-id-divider" }

                    // Right: One-time password (OTP)
                    div { class: "connect-otp-block",
                        span { class: "connect-field-label", "{tr(keys::CONNECT_LABEL_OTP, l)}" }
                        ConnectOtpInput { host_password }
                    }
                }
            }

            // ── Section: Control remote device ──
            div { class: "connect-section",
                h2 { class: "connect-section-title", "{tr(keys::CONNECT_TITLE_REMOTE, l)}" }

                // Input row: remote ID + password
                div { class: "connect-input-row",
                    div { class: "connect-input-wrap",
                        input {
                            r#type: "text",
                            class: "connect-input",
                            placeholder: "{tr(keys::CONNECT_PLACEHOLDER_ID, l)}",
                            value: "{remote_id}",
                            oninput: move |e| remote_id.set(e.value()),
                        }
                    }
                    span { class: "connect-input-sep", "{tr(keys::CONNECT_PW_SEP, l)}" }
                    div { class: "connect-input-wrap",
                        input {
                            r#type: "password",
                            class: "connect-input-short",
                            placeholder: "",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                    }
                }

                // Button row
                div { class: "connect-action-row",
                    button {
                        class: if blocked { "connect-btn connect-btn-disabled" } else { "connect-btn" },
                        disabled: blocked,
                        onclick: move |_| {
                            if !blocked {
                                let mut cfg = load_config();
                                cfg.server_addr = addr.clone();
                                cfg.remote_id   = remote_id();
                                save_config(&cfg);
                                page.set(Page::Remote);
                            }
                        },
                        "{tr(keys::CONNECT_BTN_CONNECT, l)}"
                    }
                }

                // Mode radios
                div { class: "connect-mode-row",
                    label { class: "connect-mode-label",
                        input {
                            r#type: "radio",
                            name: "conn_mode",
                            value: "desktop",
                            checked: conn_mode() == "desktop",
                            onchange: move |_| conn_mode.set("desktop".to_string()),
                        }
                        "{tr(keys::CONNECT_RADIO_DESKTOP, l)}"
                    }
                    label { class: "connect-mode-label",
                        input {
                            r#type: "radio",
                            name: "conn_mode",
                            value: "file",
                            checked: conn_mode() == "file",
                            onchange: move |_| conn_mode.set("file".to_string()),
                        }
                        "{tr(keys::CONNECT_RADIO_FILE, l)}"
                    }
                }
            }
        }
    }
}

#[component]
fn ConnectOtpInput(host_password: Signal<String>) -> Element {
    let mut show_otp = use_signal(|| false);

    rsx! {
        div { class: "connect-otp-row",
            input {
                r#type: if show_otp() { "text" } else { "password" },
                class: "connect-otp-input",
                placeholder: "",
                value: "{host_password}",
                oninput: move |e| host_password.set(e.value()),
                onblur: move |_| {
                    let mut cfg = load_config();
                    cfg.host_password = host_password();
                    save_config(&cfg);
                },
            }
            button {
                class: "connect-otp-toggle",
                onclick: move |_| show_otp.toggle(),
                if show_otp() { "\u{1F441}" } else { "\u{1F576}" }
            }
        }
    }
}

// ─── Tips card ───────────────────────────────────────────────────────────────

#[component]
pub fn TipsCard(theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let _t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        div {
            class: "tips-card",
            div {
                style: "font-size: 12px; font-weight: 600; color: var(--info); margin-bottom: 6px;",
                "{tr(keys::CONNECT_TIPS_TITLE, l)}"
            }
            div {
                class: "text-sec",
                style: "font-size: 12px; line-height: 1.6;",
                "{tr(keys::CONNECT_TIPS_L1, l)}"
                br {}
                "{tr(keys::CONNECT_TIPS_L2, l)}"
                br {}
                "{tr(keys::CONNECT_TIPS_L3, l)}"
            }
        }
    }
}

// ─── Form field helper ───────────────────────────────────────────────────────

#[component]
pub fn FormField(
    label: &'static str,
    #[props(default = "")]
    hint: &'static str,
    theme: Signal<Theme>,
    children: Element,
) -> Element {
    let _t = tc(theme());
    rsx! {
        div {
            class: "form-field",
            div {
                class: "form-field-header",
                span {
                    class: "label-form",
                    "{label}"
                }
                if !hint.is_empty() {
                    span { class: "label-hint", "{hint}" }
                }
            }
            {children}
        }
    }
}
