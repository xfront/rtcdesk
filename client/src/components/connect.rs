//! Connect tab: local device ID display, remote ID/password input, connect action.
//! Refactored with dioxus-bootstrap-css components.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::clipboard::set_system_clipboard;
use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::format_local_id;
use dioxus_bootstrap_css::theme::Theme;
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
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let mut conn_mode = use_signal(|| "desktop".to_string());
    let host_password = use_signal(move || load_config().host_password.clone());
    let rpc_client = use_context::<std::sync::Arc<std::sync::Mutex<crate::rpc_client::RpcClient>>>();

    let addr = server_addr();
    let blocked = remote_id().is_empty();

    rsx! {
        div { class: "p-4",

            // ── Section: Allow control of this device ──
            div { class: "mb-4",
                h4 { class: "mb-3", "{tr(keys::CONNECT_TITLE_LOCAL, l)}" }

                Row { class: "g-0",
                    // Left: Device ID
                    Col { md: ColumnSize::Span(8),
                        div { class: "mb-1",
                            small { class: "text-secondary", "{tr(keys::CONNECT_LABEL_LOCAL_ID, l)}" }
                        }
                        InputGroup {
                            class: "mb-2",
                            Input {
                                r#type: "text",
                                value: "{format_local_id(&local_id())}",
                                readonly: true,
                                class: "fw-bold",
                                style: "font-size: 1.2rem; letter-spacing: 2px;",
                            }
                            Button {
                                color: Color::Secondary,
                                outline: true,
                                onclick: move |_| {
                                    let id = format_local_id(&local_id());
                                    let _ = set_system_clipboard(&id);
                                },
                                "\u{1F4CB}"
                            }
                        }
                    }
                    // Divider
                    Col { md: ColumnSize::Auto,
                        div {
                            style: "width: 1px; background: var(--bs-border-color); margin: 20px 16px 4px; align-self: stretch;",
                        }
                    }
                    // Right: OTP
                    Col { md: ColumnSize::Span(3),
                        div { class: "mb-1",
                            small { class: "text-secondary", "{tr(keys::CONNECT_LABEL_OTP, l)}" }
                        }
                        ConnectOtpInput { host_password }
                    }
                }
            }

            // ── Section: Control remote device ──
            div { class: "mb-4",
                h4 { class: "mb-3", "{tr(keys::CONNECT_TITLE_REMOTE, l)}" }

                // Remote ID + password input row
                InputGroup {
                    class: "mb-3",
                    Input {
                        r#type: "text",
                        placeholder: tr(keys::CONNECT_PLACEHOLDER_ID, l),
                        value: "{remote_id}",
                        oninput: move |e: FormEvent| remote_id.set(e.value()),
                    }
                    Input {
                        r#type: "password",
                        value: "{password}",
                        placeholder: tr(keys::CONNECT_PW_SEP, l),
                        oninput: move |e: FormEvent| password.set(e.value()),
                    }
                    Button {
                        color: Color::Danger,
                        disabled: blocked,
                        class: "btn-connect",
                        onclick: move |_| {
                            if !blocked {
                                let mut cfg = load_config();
                                cfg.server_addr = addr.clone();
                                cfg.remote_id   = remote_id();
                                save_config(&cfg);
                                
                                // TODO: Initiate WebRTC connection via RPC
                                // For now, just navigate to remote page
                                // The actual connection will happen in the remote view
                                page.set(Page::Remote);
                            }
                        },
                        "{tr(keys::CONNECT_BTN_CONNECT, l)}"
                    }
                }

                // Mode radios
                div { class: "d-flex gap-3",
                    div { class: "form-check",
                        input {
                            class: "form-check-input",
                            r#type: "radio",
                            name: "conn_mode",
                            value: "desktop",
                            checked: conn_mode() == "desktop",
                            onchange: move |_| conn_mode.set("desktop".to_string()),
                        }
                        label { class: "form-check-label small", "{tr(keys::CONNECT_RADIO_DESKTOP, l)}" }
                    }
                    div { class: "form-check",
                        input {
                            class: "form-check-input",
                            r#type: "radio",
                            name: "conn_mode",
                            value: "file",
                            checked: conn_mode() == "file",
                            onchange: move |_| conn_mode.set("file".to_string()),
                        }
                        label { class: "form-check-label small", "{tr(keys::CONNECT_RADIO_FILE, l)}" }
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
        InputGroup {
            input {
                class: "form-control",
                style: "letter-spacing: 3px;",
                r#type: if show_otp() { "text" } else { "password" },
                value: "{host_password}",
                oninput: move |e| host_password.set(e.value()),
                onblur: move |_| {
                    let mut cfg = load_config();
                    cfg.host_password = host_password();
                    save_config(&cfg);
                },
            }
            Button {
                color: Color::Secondary,
                outline: true,
                onclick: move |_| show_otp.toggle(),
                if show_otp() { "\u{1F441}" } else { "\u{1F576}" }
            }
        }
    }
}

// ─── Tips card ───────────────────────────────────────────────────────────────

#[component]
pub fn TipsCard(_theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        Card {
            class: "border-info",
            body: rsx! {
                h6 { class: "text-info mb-2", "{tr(keys::CONNECT_TIPS_TITLE, l)}" }
                p { class: "text-secondary small mb-0 lh-base",
                    "{tr(keys::CONNECT_TIPS_L1, l)}"
                    br {}
                    "{tr(keys::CONNECT_TIPS_L2, l)}"
                    br {}
                    "{tr(keys::CONNECT_TIPS_L3, l)}"
                }
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
    children: Element,
) -> Element {
    rsx! {
        div { class: "mb-3",
            if !label.is_empty() {
                label { class: "form-label small fw-medium", "{label}" }
            }
            {children}
            if !hint.is_empty() {
                div { class: "form-text small", "{hint}" }
            }
        }
    }
}
