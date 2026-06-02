//! Settings tab: network config, display quality, theme, language, device identity, about.
//! Refactored with dioxus-bootstrap-css components.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::{format_local_id, generate_local_id};
use crate::quality::QUALITY_PRESETS;
use dioxus_bootstrap_css::theme::Theme;
use crate::clipboard::set_system_clipboard;

// ─── Settings tab ────────────────────────────────────────────────────────────

#[component]
pub fn SettingsTab(
    server_addr: Signal<String>,
    quality: Signal<usize>,
    theme: Signal<Theme>,
    locale: Signal<String>,
    local_id: Signal<String>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        div { class: "p-4",

            h4 { class: "mb-2", "{tr(keys::SETTINGS_TITLE, l)}" }
            p  { class: "text-secondary small mb-4", "{tr(keys::SETTINGS_SUBTITLE, l)}" }

            SettingsSection { title: tr(keys::SETTINGS_NETWORK, l),
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_SIGNAL_ADDR, l), hint: tr(keys::SETTINGS_SIGNAL_HINT, l),
                        Input {
                            r#type: "text",
                            placeholder: tr(keys::SETTINGS_PLACEHOLDER_HOST, l),
                            value: "{server_addr}",
                            oninput: move |e: FormEvent| {
                                server_addr.set(e.value());
                                let mut cfg = load_config();
                                cfg.server_addr = server_addr();
                                save_config(&cfg);
                            },
                        }
                    }

                    FormField { label: tr(keys::SETTINGS_STUN, l), hint: tr(keys::SETTINGS_STUN_HINT, l),
                        StunServersEditor { locale }
                    }
                }
            }

            SettingsSection { title: tr(keys::SETTINGS_DISPLAY, l),
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_QUALITY, l), hint: tr(keys::SETTINGS_QUALITY_HINT, l),
                        div { class: "d-flex gap-1 flex-wrap",
                            for (idx, _preset) in QUALITY_PRESETS.iter().enumerate() {
                                {
                                    let active = idx == quality().min(QUALITY_PRESETS.len() - 1);
                                    let translated = crate::i18n::quality_label(idx, l);
                                    rsx! {
                                        Button {
                                            key: "sq_{idx}",
                                            color: if active { Color::Primary } else { Color::Secondary },
                                            outline: !active,
                                            size: Size::Sm,
                                            onclick: move |_| {
                                                quality.set(idx);
                                                let mut cfg = load_config();
                                                cfg.quality = idx;
                                                save_config(&cfg);
                                            },
                                            "{translated}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    FormField { label: tr(keys::SETTINGS_THEME, l), hint: tr(keys::SETTINGS_THEME_HINT, l),
                        div { class: "d-flex align-items-center gap-2",
                            ThemeToggle { theme }
                        }
                    }

                    // Language selector
                    FormField { label: tr(keys::SETTINGS_LANG, l), hint: tr(keys::SETTINGS_LANG_HINT, l),
                        Select {
                            value: locale(),
                            onchange: move |e: dioxus::events::FormEvent| {
                                let new_locale = e.value();
                                locale.set(new_locale.clone());
                                let mut cfg = load_config();
                                cfg.locale = new_locale;
                                save_config(&cfg);
                            },
                            style: "max-width: 260px;",
                            for entry in crate::i18n::available_locales().to_vec() {
                                option {
                                    key: "{entry.id}",
                                    value: "{entry.id}",
                                    selected: locale() == entry.id,
                                    "{entry.label}"
                                }
                            }
                        }
                    }
                }
            }

            SettingsSection { title: tr(keys::SETTINGS_DEVICE_ID, l),
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_LOCAL_ID, l), hint: tr(keys::SETTINGS_LOCAL_ID_HINT, l),
                        div { class: "d-flex gap-2 align-items-center",
                            div {
                                class: "form-control fw-semibold",
                                style: "max-width: 200px; color: var(--bs-success); letter-spacing: 2px;",
                                "{format_local_id(&local_id())}"
                            }
                            Button {
                                color: Color::Secondary,
                                outline: true,
                                size: Size::Sm,
                                title: tr(keys::SETTINGS_BTN_RESET_TOOLTIP, l),
                                onclick: move |_| {
                                    let new_id = generate_local_id();
                                    local_id.set(new_id.clone());
                                    let mut cfg = load_config();
                                    cfg.local_id = new_id;
                                    save_config(&cfg);
                                },
                                "{tr(keys::SETTINGS_BTN_RESET, l)}"
                            }
                            Button {
                                color: Color::Primary,
                                outline: true,
                                size: Size::Sm,
                                onclick: move |_| {
                                    let _ = set_system_clipboard(&local_id());
                                },
                                "{tr(keys::GEN_COPY, l)}"
                            }
                        }
                    }
                }
            }

            super::auth::AuthSection { theme, locale }

            SettingsSection { title: tr(keys::SETTINGS_ABOUT, l),
                children: rsx! {
                    div { class: "d-flex align-items-center gap-3 py-2",
                        div {
                            class: "d-flex align-items-center justify-content-center rounded-3 flex-shrink-0 fw-bold",
                            style: "width: 42px; height: 42px; font-size: 20px;
                                    background: linear-gradient(135deg, var(--bs-success) 0%, var(--accent-dim) 100%);
                                    color: #fff;",
                            "R"
                        }
                        div {
                            div { class: "fw-semibold", "RTCDesk" }
                            div {
                                class: "text-secondary small mt-1",
                                "{tr(keys::SETTINGS_ABOUT_VER, l)}"
                            }
                            div {
                                class: "small mt-1",
                                style: "color: var(--bs-tertiary-color);",
                                "{tr(keys::SETTINGS_ABOUT_STACK, l)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─── Stun servers editor ─────────────────────────────────────────────────────

#[component]
fn StunServersEditor(locale: Signal<String>) -> Element {
    let loc = locale();
    let sh = tr(keys::SETTINGS_PLACEHOLDER_STUN, &loc);
    let hp = tr(keys::SETTINGS_STUN_HELP, &loc);
    let cfg = load_config();
    let stun_text = cfg.stun_servers.join(", ");
    let mut stun_input = use_signal(move || stun_text);

    rsx! {
        div {
            input {
                class: "form-control",
                r#type: "text",
                placeholder: sh,
                value: "{stun_input}",
                oninput: move |e| stun_input.set(e.value()),
                onblur: move |_| {
                    let servers: Vec<String> = stun_input()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let mut cfg = load_config();
                    cfg.stun_servers = servers;
                    save_config(&cfg);
                },
            }
            div {
                class: "small text-secondary mt-1",
                "{hp}"
            }
        }
    }
}

// ─── Settings section wrapper ─────────────────────────────────────────────────

#[component]
pub fn SettingsSection(
    title: &'static str,
    children: Element,
) -> Element {
    rsx! {
        Card { class: "mb-3",
            header: rsx! {
                span { class: "text-secondary small text-uppercase fw-semibold", "{title}" }
            },
            body: rsx! { {children} }
        }
    }
}

pub use super::connect::FormField;
