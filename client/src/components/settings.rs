//! Settings tab: network config, display quality, theme, language, device identity, about.

use dioxus::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::{format_local_id, generate_local_id};
use crate::quality::QUALITY_PRESETS;
use crate::theme::{tc, Theme};
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
    let _t = tc(theme());
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        div {
            class: "page-narrow",

            h2 {
                class: "heading-lg",
                "{tr(keys::SETTINGS_TITLE, l)}"
            }
            p {
                class: "subtitle-compact",
                "{tr(keys::SETTINGS_SUBTITLE, l)}"
            }

            SettingsSection { title: tr(keys::SETTINGS_NETWORK, l), theme,
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_SIGNAL_ADDR, l), hint: tr(keys::SETTINGS_SIGNAL_HINT, l), theme,
                        input {
                            r#type: "text",
                            class: "input",
                            placeholder: tr(keys::SETTINGS_PLACEHOLDER_HOST, l),
                            value: "{server_addr}",
                            oninput: move |e| {
                                server_addr.set(e.value());
                                let mut cfg = load_config();
                                cfg.server_addr = server_addr();
                                save_config(&cfg);
                            },
                        }
                    }

                    FormField { label: tr(keys::SETTINGS_STUN, l), hint: tr(keys::SETTINGS_STUN_HINT, l), theme,
                        StunServersEditor { theme, locale }
                    }
                }
            }

            SettingsSection { title: tr(keys::SETTINGS_DISPLAY, l), theme,
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_QUALITY, l), hint: tr(keys::SETTINGS_QUALITY_HINT, l), theme,
                        div {
                            style: "display: flex; gap: 6px; flex-wrap: wrap;",
                            for (idx, preset) in QUALITY_PRESETS.iter().enumerate() {
                                {
                                    let active = idx == quality().min(QUALITY_PRESETS.len() - 1);
                                    let translated = crate::i18n::quality_label(idx, l);
                                    rsx! {
                                        button {
                                            key: "sq_{idx}",
                                            class: if active { "selector-btn-md-active" } else { "selector-btn-md" },
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

                    FormField { label: tr(keys::SETTINGS_THEME, l), hint: tr(keys::SETTINGS_THEME_HINT, l), theme,
                        div {
                            style: "display: flex; gap: 8px; flex-wrap: wrap;",
                            for (th, label_key, icon) in [
                                (Theme::Dark,       keys::SETTINGS_THEME_DARK,        "🌙"),
                                (Theme::Light,      keys::SETTINGS_THEME_LIGHT,       "☀"),
                                (Theme::SkyBlue,    keys::SETTINGS_THEME_SKY_BLUE,    "🔵"),
                                (Theme::GrassGreen, keys::SETTINGS_THEME_GRASS_GREEN, "🌿"),
                                (Theme::Orange,     keys::SETTINGS_THEME_ORANGE,      "🍊"),
                                (Theme::Purple,     keys::SETTINGS_THEME_PURPLE,      "🟣"),
                            ] {
                                {
                                    let active = theme() == th;
                                    let label = tr(label_key, l);
                                    rsx! {
                                        button {
                                            key: "{label}",
                                            class: if active { "selector-btn-theme-active" } else { "selector-btn-theme" },
                                            onclick: move |_| {
                                                theme.set(th);
                                                let mut cfg = load_config();
                                                cfg.theme = th;
                                                save_config(&cfg);
                                            },
                                            span { style: "font-size: 14px;", "{icon}" }
                                            span { "{label}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ── Language selector ──
                    FormField { label: tr(keys::SETTINGS_LANG, l), hint: tr(keys::SETTINGS_LANG_HINT, l), theme,
                        select {
                            class: "select",
                            value: "{locale}",
                            onchange: move |e| {
                                let new_locale = e.value();
                                locale.set(new_locale.clone());
                                let mut cfg = load_config();
                                cfg.locale = new_locale;
                                save_config(&cfg);
                            },
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

            SettingsSection { title: tr(keys::SETTINGS_DEVICE_ID, l), theme,
                children: rsx! {
                    FormField { label: tr(keys::SETTINGS_LOCAL_ID, l), hint: tr(keys::SETTINGS_LOCAL_ID_HINT, l), theme,
                        div {
                            style: "display: flex; gap: 8px; align-items: center;",
                            div {
                                class: "id-display-sm",
                                "{format_local_id(&local_id())}"
                            }
                            button {
                                class: "btn-ghost",
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
                            button {
                                class: "btn-outline-accent-md",
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

            SettingsSection { title: tr(keys::SETTINGS_ABOUT, l), theme,
                children: rsx! {
                    div {
                        style: "display: flex; align-items: center; gap: 14px; padding: 12px 0;",
                        div {
                            class: "about-icon",
                            "R"
                        }
                        div {
                            div {
                                class: "text-pri",
                                style: "font-size: 15px; font-weight: 600;",
                                "RTCDesk"
                            }
                            div {
                                class: "text-sec",
                                style: "font-size: 12px; margin-top: 2px;",
                                "{tr(keys::SETTINGS_ABOUT_VER, l)}"
                            }
                            div {
                                class: "text-dim",
                                style: "font-size: 11px; margin-top: 2px;",
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
fn StunServersEditor(theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let _t = tc(theme());
    let loc = locale();
    let sh = tr(keys::SETTINGS_PLACEHOLDER_STUN, &loc);
    let hp = tr(keys::SETTINGS_STUN_HELP, &loc);
    let cfg = load_config();
    let stun_text = cfg.stun_servers.join(", ");
    let mut stun_input = use_signal(move || stun_text);

    rsx! {
        div {
            class: "flex-col",
            style: "gap: 8px; align-items: flex-start;",
            input {
                r#type: "text",
                class: "input",
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
                class: "text-dim",
                style: "font-size: 11px; line-height: 1.5;",
                "{hp}"
            }
        }
    }
}

// ─── Re-exported helpers ─────────────────────────────────────────────────────

#[component]
pub fn SettingsSection(
    title: &'static str,
    theme: Signal<Theme>,
    children: Element,
) -> Element {
    let _t = tc(theme());
    rsx! {
        div {
            class: "card-section",
            style: "margin-bottom: 16px;",
            div {
                class: "card-section-header",
                "{title}"
            }
            div { class: "card-section-body", {children} }
        }
    }
}

pub use super::connect::FormField;
