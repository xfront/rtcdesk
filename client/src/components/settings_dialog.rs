//! Settings dialog: modal overlay matching RustDesk 1.4.7 layout.
//!
//! Left sidebar: 200px wide, accent-color left bar for selected tab, icon + label.
//! Right content: scrollable list of Card widgets, each with title and children.
//!
//! Tabs: General · Security · Network · Display · Account · About

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::local_id::{format_local_id, generate_local_id};
use crate::quality::QUALITY_PRESETS;
use crate::clipboard::set_system_clipboard;
use crate::components::auth::{AuthCtx, AuthStatus};
use dioxus_bootstrap_css::theme::Theme;
use super::SettingsCard;

// ─── Build-time constants ────────────────────────────────────────────────────

const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Injected by build.rs via `CARGO_BUILD_DATE`; falls back to compile date.
const BUILD_DATE: &str = match option_env!("CARGO_BUILD_DATE") {
    Some(d) => d,
    None => "(unknown)",
};

// ─── Layout constants (matching RustDesk) ──────────────────────────────────

const _TAB_WIDTH: &str = "130px";
const _TAB_HEIGHT: &str = "42px";
const _CONTENT_FONT_SIZE: &str = "15px";

// ─── Settings tab enum ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsTabId {
    General,
    Security,
    Network,
    Display,
    Account,
    About,
}

// ─── Settings dialog overlay ───────────────────────────────────────────────

#[component]
pub fn SettingsDialog(
    show: Signal<bool>,
    theme: Signal<Theme>,
    locale: Signal<String>,
    server_addr: Signal<String>,
    quality: Signal<usize>,
    local_id: Signal<String>,
) -> Element {
    let current_tab = use_signal(|| SettingsTabId::General);
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        Modal {
            show: show,
            title: tr(keys::SETTINGS_TITLE, l).to_string(),
            size: ModalSize::Default,
            centered: true,
            backdrop_close: false,
            scrollable: false,
            show_close: true,
            class: "rtcdesk-settings-dialog",

            // Custom body: left tabs + right content
            div {
                class: "d-flex flex-grow-1",
                style: "min-height: 0;",

                // ── Left: vertical tab list (RustDesk style) ──
                div {
                    class: "d-flex flex-column flex-shrink-0",
                    style: "width: {_TAB_WIDTH}; background: var(--bs-tertiary-bg);
                            border-right: 1px solid var(--bs-border-color);
                            overflow-y: auto; padding-top: 8px;",

                    SettingsTabBtn { tab: SettingsTabId::General,  label: tr(keys::SETTINGS_TAB_GENERAL, l),  icon_unsel: "⚙", icon_sel: "⚙", current_tab }
                    SettingsTabBtn { tab: SettingsTabId::Security, label: tr(keys::SETTINGS_TAB_SECURITY, l), icon_unsel: "🔓", icon_sel: "🔒", current_tab }
                    SettingsTabBtn { tab: SettingsTabId::Network,  label: tr(keys::SETTINGS_TAB_NETWORK, l),  icon_unsel: "🌐", icon_sel: "🌐", current_tab }
                    SettingsTabBtn { tab: SettingsTabId::Display,  label: tr(keys::SETTINGS_TAB_DISPLAY, l),  icon_unsel: "🖥", icon_sel: "🖥", current_tab }
                    SettingsTabBtn { tab: SettingsTabId::Account,  label: tr(keys::SETTINGS_TAB_ACCOUNT, l),  icon_unsel: "👤", icon_sel: "👤", current_tab }
                    SettingsTabBtn { tab: SettingsTabId::About,    label: tr(keys::SETTINGS_TAB_ABOUT, l),    icon_unsel: "ℹ", icon_sel: "ℹ", current_tab }
                }

                // ── Right: content panel ────────────────────
                div {
                    class: "flex-fill overflow-y-auto",
                    style: "background: var(--bs-body-bg); padding: 0.5rem 1rem;",

                    match current_tab() {
                        SettingsTabId::General  => rsx! { GeneralPanel  { theme, locale, local_id } },
                        SettingsTabId::Security => rsx! { SecurityPanel { locale, local_id } },
                        SettingsTabId::Network  => rsx! { NetworkPanel  { locale, server_addr } },
                        SettingsTabId::Display  => rsx! { DisplayPanel  { theme, locale, quality } },
                        SettingsTabId::Account  => rsx! { AccountPanel  { theme, locale } },
                        SettingsTabId::About    => rsx! { AboutPanel    { locale, local_id } },
                    }
                }
            }
        }
    }
}

// ─── Settings tab button (RustDesk accent-bar style) ──────────────────────

#[component]
fn SettingsTabBtn(
    tab: SettingsTabId,
    label: &'static str,
    icon_unsel: &'static str,
    icon_sel: &'static str,
    current_tab: Signal<SettingsTabId>,
) -> Element {
    let active = current_tab() == tab;
    let icon = if active { icon_sel } else { icon_unsel };
    let bar_color = if active { "var(--bs-primary)" } else { "transparent" };
    let text_color = if active { "var(--bs-primary)" } else { "var(--bs-body-color)" };

    rsx! {
        div {
            class: "d-flex align-items-center",
            style: "height: {_TAB_HEIGHT}; cursor: pointer;
                    transition: all 0.15s;",
            onclick: move |_| current_tab.set(tab.clone()),

            // Left accent bar (4px wide, primary color when selected)
            div {
                style: "width: 4px; height: 70%; border-radius: 2px;
                        background: {bar_color}; margin-right: 13px;
                        transition: background 0.15s;",
            }

            // Icon
            span {
                style: "font-size: 18px; margin-right: 10px; color: {text_color};
                        transition: color 0.15s;",
                "{icon}"
            }

            // Label
            span {
                style: "font-size: {_CONTENT_FONT_SIZE}; font-weight: 400;
                        color: {text_color}; transition: color 0.15s;",
                "{label}"
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Content panels
// ═══════════════════════════════════════════════════════════════════════════

// ─── General ──────────────────────────────────────────────────────────────

#[component]
fn GeneralPanel(theme: Signal<Theme>, locale: Signal<String>, local_id: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let mut hw_codec        = use_signal(|| load_config().hw_codec);
    let mut autostart       = use_signal(|| load_config().autostart);
    let mut minimize_tray   = use_signal(|| load_config().minimize_to_tray);

    rsx! {
        div { class: "p-3",
            // Service
            SettingsCard { title: tr(keys::SETTINGS_SERVICE, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_LOCAL_ID, l)}" }
                    div { class: "d-flex gap-2 align-items-center",
                        div {
                            class: "form-control fw-semibold",
                            style: "max-width: 160px; color: var(--bs-success); letter-spacing: 2px; font-size: 13px;",
                            "{format_local_id(&local_id())}"
                        }
                        Button {
                            color: Color::Primary,
                            outline: true,
                            size: Size::Sm,
                            onclick: move |_| { let _ = set_system_clipboard(&local_id()); },
                            "{tr(keys::GEN_COPY, l)}"
                        }
                    }
                }
            }

            // Theme
            SettingsCard { title: tr(keys::SETTINGS_THEME, l),
                div { class: "px-3 py-1",
                    ThemeToggle { theme }
                }
            }

            // Language
            SettingsCard { title: tr(keys::SETTINGS_LANG, l),
                div { class: "px-3 py-2",
                    Select {
                        value: locale(),
                        onchange: move |e: FormEvent| {
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

            // Hardware Codec
            SettingsCard { title: tr(keys::SETTINGS_HW_CODEC, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_HW_CODEC_ENABLE, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: hw_codec(),
                        onchange: move |_| {
                            hw_codec.toggle();
                            let mut cfg = load_config();
                            cfg.hw_codec = hw_codec();
                            save_config(&cfg);
                        },
                    }
                }
            }

            // Other
            SettingsCard { title: tr(keys::SETTINGS_OTHER, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_AUTOSTART, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: autostart(),
                        onchange: move |_| {
                            autostart.toggle();
                            let mut cfg = load_config();
                            cfg.autostart = autostart();
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_MINIMIZE_TO_TRAY, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: minimize_tray(),
                        onchange: move |_| {
                            minimize_tray.toggle();
                            let mut cfg = load_config();
                            cfg.minimize_to_tray = minimize_tray();
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_CONFIRM_CLOSE_TABS, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: true,
                        readonly: true,
                    }
                }
            }
        }
    }
}

// ─── Security ─────────────────────────────────────────────────────────────

#[component]
fn SecurityPanel(locale: Signal<String>, local_id: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let perm_keyboard    = use_signal(|| load_config().perm_keyboard.unwrap_or(true));
    let perm_clipboard   = use_signal(|| load_config().perm_clipboard.unwrap_or(true));
    let perm_file        = use_signal(|| load_config().perm_file.unwrap_or(true));
    let perm_audio       = use_signal(|| load_config().perm_audio.unwrap_or(false));
    let mut access_mode          = use_signal(|| load_config().access_mode.clone());
    let mut otp_len              = use_signal(|| load_config().otp_len);
    let mut otp_digits_only      = use_signal(|| load_config().otp_digits_only);
    let mut lock_on_session_end  = use_signal(|| load_config().lock_on_session_end);
    let mut deny_lan             = use_signal(|| load_config().deny_lan_discovery);
    let mut approve_interactive  = use_signal(|| load_config().approve_mode_interactive);
    // show/hide the set-permanent-password dialog
    let mut show_set_pw_dialog   = use_signal(|| false);

    rsx! {
        div { class: "p-3",
            // Permissions
            SettingsCard { title: tr(keys::SETTINGS_PERMISSIONS, l),
                PermToggle { label: tr(keys::SETTINGS_PERM_KEYBOARD, l),  checked: perm_keyboard,  field: "perm_keyboard" }
                PermToggle { label: tr(keys::SETTINGS_PERM_CLIPBOARD, l), checked: perm_clipboard, field: "perm_clipboard" }
                PermToggle { label: tr(keys::SETTINGS_PERM_FILE, l),      checked: perm_file,      field: "perm_file" }
                PermToggle { label: tr(keys::SETTINGS_PERM_AUDIO, l),     checked: perm_audio,     field: "perm_audio" }
            }

            // Password
            SettingsCard { title: tr(keys::SETTINGS_PASSWORD, l),

                // ── Access-mode dropdown (top of card) ─────────────────────────────
                div { class: "px-3 pt-2 pb-1",
                    select {
                        class: "form-select form-select-sm",
                        value: "{access_mode}",
                        onchange: move |e: FormEvent| {
                            access_mode.set(e.value());
                            let mut cfg = load_config();
                            cfg.access_mode = access_mode();
                            save_config(&cfg);
                        },
                        option { value: "password_otp",       "{tr(keys::SETTINGS_ACCESS_MODE_OTP, l)}" }
                        option { value: "password_permanent", "{tr(keys::SETTINGS_ACCESS_MODE_PERMANENT, l)}" }
                        option { value: "password_both",      "{tr(keys::SETTINGS_ACCESS_MODE_BOTH, l)}" }
                        option { value: "click_only",         "{tr(keys::SETTINGS_ACCESS_MODE_CLICK, l)}" }
                    }
                }

                // ── Radio: Use one-time password ────────────────────────────────
                div { class: "px-3 py-1 d-flex align-items-center gap-2",
                    input {
                        r#type: "radio",
                        class: "form-check-input flex-shrink-0",
                        name: "pw_mode",
                        checked: access_mode() == "password_otp" || access_mode() == "password_both",
                        onchange: move |_| {
                            // clicking OTP radio sets to otp-only (unless "both" already)
                            if access_mode() != "password_both" {
                                access_mode.set("password_otp".to_string());
                                let mut cfg = load_config();
                                cfg.access_mode = "password_otp".to_string();
                                save_config(&cfg);
                            }
                        },
                    }
                    span { style: "font-size: {_CONTENT_FONT_SIZE};",
                        "{tr(keys::SETTINGS_OTP_USE, l)}"
                    }
                }

                // OTP sub-options (only visible when OTP is selected)
                if access_mode() == "password_otp" || access_mode() == "password_both" {
                    // OTP length: 6 / 8 / 10
                    div { class: "d-flex align-items-center gap-3 px-3 ps-5 py-1",
                        span { style: "font-size: 13px; color: var(--bs-secondary-color);",
                            "{tr(keys::SETTINGS_OTP_LEN, l)}"
                        }
                        for len_val in [6u8, 8, 10] {
                            {
                                let is_sel = otp_len() == len_val;
                                rsx! {
                                    label {
                                        class: "d-flex align-items-center gap-1",
                                        style: "cursor: pointer; font-size: 13px;",
                                        input {
                                            r#type: "radio",
                                            class: "form-check-input",
                                            name: "otp_len",
                                            checked: is_sel,
                                            onchange: move |_| {
                                                otp_len.set(len_val);
                                                let mut cfg = load_config();
                                                cfg.otp_len = len_val;
                                                save_config(&cfg);
                                            },
                                        }
                                        "{len_val}"
                                    }
                                }
                            }
                        }
                    }
                    // OTP digits only
                    div { class: "d-flex align-items-center gap-2 px-3 ps-5 py-1",
                        input {
                            r#type: "checkbox",
                            class: "form-check-input",
                            id: "otp_digits_only",
                            checked: otp_digits_only(),
                            onchange: move |_| {
                                otp_digits_only.toggle();
                                let mut cfg = load_config();
                                cfg.otp_digits_only = otp_digits_only();
                                save_config(&cfg);
                            },
                        }
                        label {
                            r#for: "otp_digits_only",
                            style: "font-size: 13px; color: var(--bs-secondary-color); cursor: pointer;",
                            "{tr(keys::SETTINGS_OTP_DIGITS_ONLY, l)}"
                        }
                    }
                }

                // ── Radio: Use permanent password ─────────────────────────────
                div { class: "px-3 py-1 d-flex align-items-center gap-2",
                    input {
                        r#type: "radio",
                        class: "form-check-input flex-shrink-0",
                        name: "pw_mode",
                        checked: access_mode() == "password_permanent" || access_mode() == "password_both",
                        onchange: move |_| {
                            if access_mode() != "password_both" {
                                access_mode.set("password_permanent".to_string());
                                let mut cfg = load_config();
                                cfg.access_mode = "password_permanent".to_string();
                                save_config(&cfg);
                            }
                        },
                    }
                    span { style: "font-size: {_CONTENT_FONT_SIZE};",
                        "{tr(keys::SETTINGS_PERM_USE, l)}"
                    }
                }

                // "Set Permanent Password" button (visible when permanent mode is selected)
                if access_mode() == "password_permanent" || access_mode() == "password_both" {
                    div { class: "px-3 ps-5 py-1",
                        Button {
                            color: Color::Primary,
                            size: Size::Sm,
                            onclick: move |_| show_set_pw_dialog.set(true),
                            "{tr(keys::SETTINGS_PERM_SET_BTN, l)}"
                        }
                    }
                }

                // ── Radio: Use both passwords ──────────────────────────────────
                div { class: "px-3 py-1 pb-2 d-flex align-items-center gap-2",
                    input {
                        r#type: "radio",
                        class: "form-check-input flex-shrink-0",
                        name: "pw_mode",
                        checked: access_mode() == "password_both",
                        onchange: move |_| {
                            access_mode.set("password_both".to_string());
                            let mut cfg = load_config();
                            cfg.access_mode = "password_both".to_string();
                            save_config(&cfg);
                        },
                    }
                    span { style: "font-size: {_CONTENT_FONT_SIZE};",
                        "{tr(keys::SETTINGS_BOTH_USE, l)}"
                    }
                }
            }

            // ID
            SettingsCard { title: tr(keys::SETTINGS_ID_SECTION, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_LOCAL_ID, l)}" }
                    div { class: "d-flex gap-2 align-items-center",
                        div {
                            class: "form-control form-control-sm fw-semibold",
                            style: "max-width: 160px; color: var(--bs-success); letter-spacing: 2px;",
                            "{format_local_id(&local_id())}"
                        }
                        Button {
                            color: Color::Primary,
                            outline: true,
                            size: Size::Sm,
                            onclick: move |_| {
                                let new_id = generate_local_id();
                                local_id.set(new_id.clone());
                                let mut cfg = load_config();
                                cfg.local_id = new_id;
                                save_config(&cfg);
                            },
                            "{tr(keys::SETTINGS_BTN_RESET, l)}"
                        }
                    }
                }
            }

            // Advanced security
            SettingsCard { title: tr(keys::SETTINGS_SECURITY, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_LOCK_ON_SESSION_END, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: lock_on_session_end(),
                        onchange: move |_| {
                            lock_on_session_end.toggle();
                            let mut cfg = load_config();
                            cfg.lock_on_session_end = lock_on_session_end();
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_DENY_LAN_DISCOVERY, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: deny_lan(),
                        onchange: move |_| {
                            deny_lan.toggle();
                            let mut cfg = load_config();
                            cfg.deny_lan_discovery = deny_lan();
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_APPROVE_MODE_INTERACTIVE, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: approve_interactive(),
                        onchange: move |_| {
                            approve_interactive.toggle();
                            let mut cfg = load_config();
                            cfg.approve_mode_interactive = approve_interactive();
                            save_config(&cfg);
                        },
                    }
                }
            }
        }

        // Set-permanent-password dialog
        SetPermanentPasswordDialog {
            show: show_set_pw_dialog,
            locale,
        }
    }
}

// ─── Set Permanent Password Dialog ───────────────────────────────────────────

#[component]
fn SetPermanentPasswordDialog(
    show: Signal<bool>,
    locale: Signal<String>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let mut new_pw      = use_signal(|| String::new());
    let mut confirm_pw  = use_signal(|| String::new());
    let mut error_msg   = use_signal(|| String::new());
    let mut saved       = use_signal(|| false);

    // reset state when dialog opens
    let show_val = show();
    use_effect(move || {
        if show_val {
            new_pw.set(String::new());
            confirm_pw.set(String::new());
            error_msg.set(String::new());
            saved.set(false);
        }
    });

    let mut on_save = move || {
        let pw = new_pw();
        let cpw = confirm_pw();
        if pw.is_empty() {
            error_msg.set(tr(keys::SETTINGS_SET_PERM_PW_EMPTY, l).to_string());
            return;
        }
        if pw != cpw {
            error_msg.set(tr(keys::SETTINGS_SET_PERM_PW_MISMATCH, l).to_string());
            return;
        }
        let mut cfg = load_config();
        cfg.host_password = pw;
        save_config(&cfg);
        error_msg.set(String::new());
        saved.set(true);
    };

    let on_clear = move |_| {
        let mut cfg = load_config();
        cfg.host_password = String::new();
        save_config(&cfg);
        new_pw.set(String::new());
        confirm_pw.set(String::new());
        error_msg.set(String::new());
        saved.set(true);
    };

    rsx! {
        Modal {
            show: show,
            title: tr(keys::SETTINGS_SET_PERM_PW_TITLE, l).to_string(),
            size: ModalSize::Sm,
            centered: true,
            backdrop_close: true,
            show_close: true,

            div { class: "px-1 py-1",
                // Hint text
                p {
                    class: "text-secondary small mb-3",
                    "{tr(keys::SETTINGS_SET_PERM_PW_HINT, l)}"
                }

                // New password
                div { class: "mb-2",
                    label { class: "form-label small mb-1",
                        "{tr(keys::SETTINGS_SET_PERM_PW_NEW, l)}"
                    }
                    input {
                        class: "form-control form-control-sm",
                        r#type: "password",
                        value: "{new_pw}",
                        oninput: move |e| {
                            new_pw.set(e.value());
                            error_msg.set(String::new());
                            saved.set(false);
                        },
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter { on_save() }
                        },
                    }
                }

                // Confirm password
                div { class: "mb-2",
                    label { class: "form-label small mb-1",
                        "{tr(keys::SETTINGS_SET_PERM_PW_CONFIRM, l)}"
                    }
                    input {
                        class: "form-control form-control-sm",
                        r#type: "password",
                        value: "{confirm_pw}",
                        oninput: move |e| {
                            confirm_pw.set(e.value());
                            error_msg.set(String::new());
                            saved.set(false);
                        },
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter { on_save() }
                        },
                    }
                }

                // Error message
                if !error_msg().is_empty() {
                    div {
                        class: "text-danger small mb-2",
                        "{error_msg}"
                    }
                }

                // Success message
                if saved() {
                    div {
                        class: "text-success small mb-2",
                        "{tr(keys::SETTINGS_SET_PERM_PW_SAVED, l)}"
                    }
                }

                // Action buttons
                div { class: "d-flex gap-2 mt-3",
                    Button {
                        color: Color::Primary,
                        class: "flex-grow-1",
                        onclick: move|e|{ on_save(); },
                        "{tr(keys::SETTINGS_SET_PERM_PW_SAVE, l)}"
                    }
                    Button {
                        color: Color::Secondary,
                        outline: true,
                        onclick: on_clear,
                        "{tr(keys::SETTINGS_SET_PERM_PW_CLEAR, l)}"
                    }
                }
            }
        }
    }
}

// ─── Network ──────────────────────────────────────────────────────────────

#[component]
fn NetworkPanel(locale: Signal<String>, server_addr: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let mut proxy_host    = use_signal(move || load_config().proxy_host.clone());
    let mut relay_server  = use_signal(move || load_config().relay_server.clone());
    let mut bandwidth     = use_signal(move || load_config().bandwidth_kbps.to_string());

    rsx! {
        div { class: "p-3",
            // ID/Relay Server
            SettingsCard { title: tr(keys::SETTINGS_ID_RELAY_SERVER, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_SIGNAL_ADDR, l)}" }
                    input {
                        class: "form-control form-control-sm",
                        style: "max-width: 260px;",
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
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_RELAY_SERVER, l)}" }
                    input {
                        class: "form-control form-control-sm",
                        style: "max-width: 260px;",
                        r#type: "text",
                        placeholder: tr(keys::SETTINGS_RELAY_SERVER_HINT, l),
                        value: "{relay_server}",
                        oninput: move |e| relay_server.set(e.value()),
                        onblur: move |_| {
                            let mut cfg = load_config();
                            cfg.relay_server = relay_server();
                            save_config(&cfg);
                        },
                    }
                }
            }

            // Proxy
            SettingsCard { title: tr(keys::SETTINGS_PROXY, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_PROXY_HOST, l)}" }
                    input {
                        class: "form-control form-control-sm",
                        style: "max-width: 260px;",
                        r#type: "text",
                        placeholder: tr(keys::SETTINGS_PROXY_HINT, l),
                        value: "{proxy_host}",
                        oninput: move |e| proxy_host.set(e.value()),
                        onblur: move |_| {
                            let mut cfg = load_config();
                            cfg.proxy_host = proxy_host();
                            save_config(&cfg);
                        },
                    }
                }
            }

            // STUN
            SettingsCard { title: tr(keys::SETTINGS_STUN, l),
                StunServersEditor { locale }
            }

            // Bandwidth
            SettingsCard { title: tr(keys::SETTINGS_BANDWIDTH, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_BANDWIDTH_KBPS, l)}" }
                    input {
                        class: "form-control form-control-sm",
                        style: "max-width: 120px;",
                        r#type: "number",
                        min: "0",
                        placeholder: "0",
                        value: "{bandwidth}",
                        oninput: move |e| bandwidth.set(e.value()),
                        onblur: move |_| {
                            let kbps: u32 = bandwidth().parse().unwrap_or(0);
                            bandwidth.set(kbps.to_string());
                            let mut cfg = load_config();
                            cfg.bandwidth_kbps = kbps;
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "px-3 pb-2",
                    span { class: "small text-secondary", "{tr(keys::SETTINGS_BANDWIDTH_HINT, l)}" }
                }
            }
        }
    }
}

// ─── Display ──────────────────────────────────────────────────────────────

#[component]
fn DisplayPanel(theme: Signal<Theme>, locale: Signal<String>, quality: Signal<usize>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let view_style            = use_signal(|| load_config().view_style.clone());
    let scroll_style          = use_signal(|| load_config().scroll_style.clone());
    let mut sync_clipboard         = use_signal(|| load_config().sync_clipboard.unwrap_or(true));
    let mut show_remote_cursor     = use_signal(|| load_config().show_remote_cursor.unwrap_or(true));
    let codec_pref            = use_signal(|| load_config().codec_pref.clone());
    let mut show_monitors_toolbar  = use_signal(|| load_config().show_monitors_toolbar.unwrap_or(true));
    let mut show_magnifier         = use_signal(|| load_config().show_magnifier.unwrap_or(false));
    let mut disable_animations     = use_signal(|| load_config().disable_animations.unwrap_or(false));

    rsx! {
        div { class: "p-3",
            // Default View Style
            SettingsCard { title: tr(keys::SETTINGS_VIEW_STYLE, l),
                SettingsRadio { label: tr(keys::SETTINGS_VIEW_ORIGINAL, l),  value: "original",  group: view_style,  field: "view_style" }
                SettingsRadio { label: tr(keys::SETTINGS_VIEW_ADAPTIVE, l),  value: "adaptive",  group: view_style,  field: "view_style" }
            }

            // Default Scroll Style
            SettingsCard { title: tr(keys::SETTINGS_SCROLL_STYLE, l),
                SettingsRadio { label: tr(keys::SETTINGS_SCROLL_AUTO, l), value: "auto", group: scroll_style, field: "scroll_style" }
                SettingsRadio { label: tr(keys::SETTINGS_SCROLL_BAR, l), value: "bar",  group: scroll_style, field: "scroll_style" }
            }

            // Default Image Quality
            SettingsCard { title: tr(keys::SETTINGS_IMAGE_QUALITY, l),
                for (idx, _preset) in QUALITY_PRESETS.iter().enumerate() {
                    {
                        let quality_key = match idx {
                            0 => tr(keys::SETTINGS_QUALITY_LOW, l),
                            1 => tr(keys::SETTINGS_QUALITY_BALANCED, l),
                            2 => tr(keys::SETTINGS_QUALITY_BEST, l),
                            _ => tr(keys::SETTINGS_QUALITY_CUSTOM, l),
                        };
                        let active = idx == quality().min(QUALITY_PRESETS.len() - 1);
                        let color = if active { "var(--bs-primary)" } else { "var(--bs-body-color)" };
                        rsx! {
                            div {
                                key: "q_{idx}",
                                class: "d-flex justify-content-between align-items-center px-3 py-1",
                                style: "cursor: pointer;",
                                onclick: move |_| {
                                    quality.set(idx);
                                    let mut cfg = load_config();
                                    cfg.quality = idx;
                                    save_config(&cfg);
                                },
                                span { style: "font-size: {_CONTENT_FONT_SIZE}; color: {color};", "{quality_key}" }
                                if active {
                                    span { style: "color: var(--bs-primary);", "●" }
                                }
                            }
                        }
                    }
                }
            }

            // Codec Preference
            SettingsCard { title: tr(keys::SETTINGS_CODEC, l),
                SettingsRadio { label: tr(keys::SETTINGS_CODEC_AUTO, l), value: "auto", group: codec_pref, field: "codec_pref" }
                SettingsRadio { label: tr(keys::SETTINGS_CODEC_H264, l), value: "h264", group: codec_pref, field: "codec_pref" }
                SettingsRadio { label: tr(keys::SETTINGS_CODEC_H265, l), value: "h265", group: codec_pref, field: "codec_pref" }
                SettingsRadio { label: tr(keys::SETTINGS_CODEC_VP9, l), value: "vp9",  group: codec_pref, field: "codec_pref" }
            }

            // Other
            SettingsCard { title: tr(keys::SETTINGS_DISPLAY_OTHER, l),
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_SYNC_CLIPBOARD, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: sync_clipboard(),
                        onchange: move |_| {
                            sync_clipboard.toggle();
                            let mut cfg = load_config();
                            cfg.sync_clipboard = Some(sync_clipboard());
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_SHOW_REMOTE_CURSOR, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: show_remote_cursor(),
                        onchange: move |_| {
                            show_remote_cursor.toggle();
                            let mut cfg = load_config();
                            cfg.show_remote_cursor = Some(show_remote_cursor());
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_SHOW_MONITORS_TOOLBAR, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: show_monitors_toolbar(),
                        onchange: move |_| {
                            show_monitors_toolbar.toggle();
                            let mut cfg = load_config();
                            cfg.show_monitors_toolbar = Some(show_monitors_toolbar());
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_SHOW_MAGNIFIER, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: show_magnifier(),
                        onchange: move |_| {
                            show_magnifier.toggle();
                            let mut cfg = load_config();
                            cfg.show_magnifier = Some(show_magnifier());
                            save_config(&cfg);
                        },
                    }
                }
                div { class: "d-flex justify-content-between align-items-center px-3 py-2",
                    span { style: "font-size: {_CONTENT_FONT_SIZE};", "{tr(keys::SETTINGS_DISABLE_ANIMATIONS, l)}" }
                    input {
                        r#type: "checkbox",
                        class: "form-check-input",
                        checked: disable_animations(),
                        onchange: move |_| {
                            disable_animations.toggle();
                            let mut cfg = load_config();
                            cfg.disable_animations = Some(disable_animations());
                            save_config(&cfg);
                        },
                    }
                }
            }
        }
    }
}

// ─── Account ──────────────────────────────────────────────────────────────

#[component]
fn AccountPanel(theme: Signal<Theme>, locale: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let AuthCtx { status, .. } = use_context::<AuthCtx>();
    let is_logged_in = matches!(status(), AuthStatus::LoggedIn { .. });
    let display_name = match status() {
        AuthStatus::LoggedIn { ref username } => username.clone(),
        _ => String::new(),
    };
    let avatar_letter = display_name.chars().next().unwrap_or('?').to_uppercase().to_string();

    rsx! {
        div { class: "p-3",
            // When logged in: show user card + logout via AuthSection
            if is_logged_in {
                SettingsCard { title: tr(keys::SETTINGS_ACCOUNT, l),
                    div {
                        class: "d-flex align-items-center gap-3 mx-3 mt-1 mb-2 p-3 rounded-3",
                        style: "background: var(--bs-tertiary-bg);",
                        div {
                            class: "d-flex align-items-center justify-content-center rounded-circle flex-shrink-0",
                            style: "width: 44px; height: 44px;
                                    background: var(--bs-primary); color: white;
                                    font-size: 20px; font-weight: 600;",
                            "{avatar_letter}"
                        }
                        div { class: "flex-fill",
                            div { class: "fw-semibold", style: "font-size: 15px;", "{display_name}" }
                            div {
                                class: "small mt-1",
                                style: "color: var(--bs-secondary-color);",
                                "@{display_name}"
                            }
                        }
                    }
                }
            }
            // Auth section handles both login form (logged out) and logout button (logged in)
            super::auth::AuthSection { theme, locale }
        }
    }
}

// ─── About ────────────────────────────────────────────────────────────────

#[component]
fn AboutPanel(locale: Signal<String>, local_id: Signal<String>) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let fp = local_id();

    rsx! {
        div { class: "p-3",
            SettingsCard { title: tr(keys::SETTINGS_ABOUT_TITLE, l),
                div { class: "px-3 py-1",
                    div { class: "py-1",
                        span { style: "font-size: {_CONTENT_FONT_SIZE};",
                            "{tr(keys::SETTINGS_ABOUT_VER, l)} {BUILD_VERSION}"
                        }
                    }
                    div { class: "py-1",
                        span { style: "font-size: {_CONTENT_FONT_SIZE};",
                            "{tr(keys::SETTINGS_ABOUT_BUILD_DATE, l)} {BUILD_DATE}"
                        }
                    }
                    div { class: "py-1",
                        span { style: "font-size: {_CONTENT_FONT_SIZE};",
                            "{tr(keys::SETTINGS_ABOUT_FINGERPRINT, l)} {fp}"
                        }
                    }
                    div { class: "py-1",
                        a {
                            style: "font-size: {_CONTENT_FONT_SIZE}; text-decoration: underline; color: var(--bs-primary);",
                            "{tr(keys::SETTINGS_ABOUT_PRIVACY, l)}"
                        }
                    }
                    div { class: "py-1",
                        a {
                            href: "https://github.com/rtcdesk",
                            target: "_blank",
                            style: "font-size: {_CONTENT_FONT_SIZE}; text-decoration: underline; color: var(--bs-primary);",
                            "{tr(keys::SETTINGS_ABOUT_WEBSITE, l)}"
                        }
                    }

                    // Copyright banner
                    div {
                        class: "mt-3 p-3 rounded-2",
                        style: "background: var(--bs-primary); color: white;",
                        div {
                            style: "font-size: 13px;",
                            "{tr(keys::SETTINGS_ABOUT_COPYRIGHT, l)}"
                        }
                    }
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Shared sub-components
// ═══════════════════════════════════════════════════════════════════════════

/// Permission toggle row (label on left, checkbox on right).
#[component]
fn PermToggle(label: &'static str, checked: Signal<bool>, field: &'static str) -> Element {
    rsx! {
        div { class: "d-flex justify-content-between align-items-center px-3 py-2",
            span { style: "font-size: {_CONTENT_FONT_SIZE};", "{label}" }
            input {
                r#type: "checkbox",
                class: "form-check-input",
                checked: checked(),
                onchange: move |_| {
                    checked.toggle();
                    let mut cfg = load_config();
                    match field {
                        "perm_keyboard"  => cfg.perm_keyboard = Some(checked()),
                        "perm_clipboard" => cfg.perm_clipboard = Some(checked()),
                        "perm_file"      => cfg.perm_file = Some(checked()),
                        "perm_audio"     => cfg.perm_audio = Some(checked()),
                        _ => {}
                    }
                    save_config(&cfg);
                },
            }
        }
    }
}

/// Radio button row for settings options.
#[component]
fn SettingsRadio(label: &'static str, value: &'static str, group: Signal<String>, field: &'static str) -> Element {
    let active = group() == value;
    let color = if active { "var(--bs-primary)" } else { "var(--bs-body-color)" };

    rsx! {
        div {
            class: "d-flex justify-content-between align-items-center px-3 py-1",
            style: "cursor: pointer;",
            onclick: move |_| {
                group.set(value.to_string());
                let mut cfg = load_config();
                match field {
                    "view_style"   => cfg.view_style = value.to_string(),
                    "scroll_style" => cfg.scroll_style = value.to_string(),
                    "codec_pref"   => cfg.codec_pref = value.to_string(),
                    _ => {}
                }
                save_config(&cfg);
            },
            span {
                style: "font-size: {_CONTENT_FONT_SIZE}; color: {color};",
                "{label}"
            }
            if active {
                span { style: "color: var(--bs-primary);", "●" }
            }
        }
    }
}

// ─── Stun servers editor ──────────────────────────────────────────────────

#[component]
fn StunServersEditor(locale: Signal<String>) -> Element {
    let loc = locale();
    let sh = tr(keys::SETTINGS_PLACEHOLDER_STUN, &loc);
    let hp = tr(keys::SETTINGS_STUN_HELP, &loc);
    let cfg = load_config();
    let stun_text = cfg.stun_servers.join(", ");
    let mut stun_input = use_signal(move || stun_text);

    rsx! {
        div { class: "px-3 py-2",
            input {
                class: "form-control form-control-sm",
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
