//! Internationalization (i18n) module — dynamic locale loading.
//!
//! Translations are discovered at runtime by scanning `conf/*.ini` in the
//! directory that contains the RTCDesk executable. Each `.ini` file maps to
//! one locale; the `i18n.locale` key holds the BCP 47 language tag (e.g.
//! `"en-US"`, `"zh-CN"`) used as the locale id. Falls back to the filename
//! stem if the key is missing.
//!
//! ## Special keys
//!
//! | Key          | Purpose                                 |
//! |--------------|-----------------------------------------|
//! | `i18n.locale` | BCP 47 language tag for this locale    |
//! | `i18n.label`  | Display name shown in the locale picker |
//!
//! ## Adding a new locale
//!
//! Drop a `.ini` file into the `conf/` directory next to the binary — no
//! recompilation needed. Must define `i18n.locale` (e.g. `i18n.locale = fr-FR`).
//!
//! ```rust,ignore
//! use crate::i18n::t;
//! let text = t(keys::APP_TITLE, "en-US"); // "RTCDesk"
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use crate::quality::{ScaleMode, QUALITY_PRESETS};

// ─── Locale entry (for building the UI selector) ──────────────────────────────

/// One discovered locale.
#[derive(Debug, Clone)]
pub struct LocaleEntry {
    /// Locale identifier (BCP 47 language tag, from `i18n.locale` key).
    pub id: String,
    /// Human-readable label (from the `i18n.label` key inside the file).
    pub label: String,
}

// ─── Internal loaded data ─────────────────────────────────────────────────────

struct LocaleData {
    entries: Vec<LocaleEntry>,
    maps: HashMap<String, HashMap<String, String>>,
    default_id: String,
}

static DATA: LazyLock<LocaleData> = LazyLock::new(load_all);

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_default()
}

fn load_all() -> LocaleData {
    let conf_dir = exe_dir().join("conf");
    let mut entries: Vec<LocaleEntry> = Vec::new();
    let mut maps: HashMap<String, HashMap<String, String>> = HashMap::new();

    // Helper – extract locale id from parsed map, fall back to filename stem.
    let resolve_id = |map: &HashMap<String, String>, file_stem: &str| {
        map.get("i18n.locale")
            .cloned()
            .unwrap_or_else(|| file_stem.to_string())
    };

    // 1 — Scan conf/*.ini next to the executable
    if let Ok(dir) = std::fs::read_dir(&conf_dir) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "ini") {
                let file_stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    let map = parse_ini(&raw);
                    let id = resolve_id(&map, &file_stem);
                    let label = map
                        .get("i18n.label")
                        .cloned()
                        .unwrap_or_else(|| id.clone());
                    entries.push(LocaleEntry {
                        id: id.clone(),
                        label,
                    });
                    maps.insert(id, map);
                }
            }
        }
    }

    // 2 — Fallback: embedded English + Chinese when conf/ has nothing
    if entries.is_empty() {
        let en_raw = include_str!("assets/locales/English.ini");
        let en_map = parse_ini(en_raw);
        let en_id = resolve_id(&en_map, "English");
        entries.push(LocaleEntry {
            id: en_id.clone(),
            label: en_map
                .get("i18n.label")
                .cloned()
                .unwrap_or_else(|| "English".into()),
        });
        maps.insert(en_id, en_map);

        let zh_raw = include_str!("assets/locales/Chinese.ini");
        let zh_map = parse_ini(zh_raw);
        let zh_id = resolve_id(&zh_map, "Chinese");
        entries.push(LocaleEntry {
            id: zh_id.clone(),
            label: zh_map
                .get("i18n.label")
                .cloned()
                .unwrap_or_else(|| "中文".into()),
        });
        maps.insert(zh_id, zh_map);
    }

    let default_id = entries.first().map(|e| e.id.clone()).unwrap_or_default();

    LocaleData {
        entries,
        maps,
        default_id,
    }
}

// ─── INI parser ──────────────────────────────────────────────────────────────

fn parse_ini(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// All discovered locales (for building the UI picker).
pub fn available_locales() -> &'static [LocaleEntry] {
    &DATA.entries
}

/// The id of the first discovered locale (used as the initial default).
pub fn default_locale_id() -> &'static str {
    &DATA.default_id
}

// ─── Translation functions ────────────────────────────────────────────────────

/// Look up a translation by key and locale id.
///
/// Falls back to the default locale, then to `"¿key?"` when the key is unknown.
pub fn t(key: &str, locale_id: &str) -> &'static str {
    let data = &*DATA;
    let map = data
        .maps
        .get(locale_id)
        .or_else(|| data.maps.get(&data.default_id));
    map.and_then(|m| m.get(key))
        .map(String::as_str)
        .unwrap_or("¿key?")
}

/// Look up a translation and replace `{var}` placeholders.
///
/// ```rust,ignore
/// use crate::i18n::tf;
/// let s = tf(keys::SIDEBAR_LOGGED_IN, "en-US", &[("username", "admin")]);
/// ```
pub fn tf(key: &str, locale_id: &str, vars: &[(&str, &str)]) -> String {
    let mut s = t(key, locale_id).to_string();
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

// ─── Quality / Scale helpers ─────────────────────────────────────────────────

/// Get translated label for a quality preset index.
pub fn quality_label(idx: usize, locale_id: &str) -> &'static str {
    match idx {
        0 => t(keys::QUALITY_BEST, locale_id),
        1 => t(keys::QUALITY_BALANCED, locale_id),
        2 => t(keys::QUALITY_SMOOTH, locale_id),
        3 => t(keys::QUALITY_LOW_BW, locale_id),
        _ => QUALITY_PRESETS[0].1,
    }
}

/// Get translated label for a `ScaleMode`.
pub fn scale_label(mode: ScaleMode, locale_id: &str) -> &'static str {
    match mode {
        ScaleMode::Fit => t(keys::SCALE_FIT, locale_id),
        ScaleMode::Stretch => t(keys::SCALE_STRETCH, locale_id),
        ScaleMode::Original => t(keys::SCALE_ORIGINAL, locale_id),
    }
}

// ─── Key constants (compile-time checked, used in components) ────────────────

#[allow(dead_code, non_upper_case_globals)]
pub mod keys {
    // App / Titlebar
    pub const APP_TITLE: &str = "app.title";
    pub const APP_VERSION: &str = "app.version";
    pub const TITLE_MINIMIZE: &str = "title.minimize";
    pub const TITLE_MAXIMIZE: &str = "title.maximize";
    pub const TITLE_CLOSE: &str = "title.close";
    pub const TITLE_LIGHT_MODE: &str = "title.light_mode";
    pub const TITLE_DARK_MODE: &str = "title.dark_mode";

    // Titlebar menu
    pub const MENU_SETTINGS: &str = "menu.settings";
    pub const MENU_LOCK_UI: &str = "menu.lock_ui";
    pub const MENU_LOGOUT: &str = "menu.logout";
    pub const MENU_EXIT: &str = "menu.exit";

    // Sidebar
    pub const SIDEBAR_DEVICES: &str = "sidebar.devices";
    pub const SIDEBAR_REMOTE_CTRL: &str = "sidebar.remote_ctrl";
    pub const SIDEBAR_HOST_SERVICE: &str = "sidebar.host_service";
    pub const SIDEBAR_SETTINGS: &str = "sidebar.settings";
    pub const SIDEBAR_LOGGED_IN: &str = "sidebar.logged_in";
    pub const SIDEBAR_NOT_LOGGED_IN: &str = "sidebar.not_logged_in";
    pub const SIDEBAR_LOGGED_IN_SHORT: &str = "sidebar.logged_in_short";
    pub const SIDEBAR_NOT_LOGGED_IN_SHORT: &str = "sidebar.not_logged_in_short";
    pub const SIDEBAR_SECURE_LINK: &str = "sidebar.secure_link";
    pub const SIDEBAR_LOCK_SET_TITLE: &str = "sidebar.lock_set_title";
    pub const SIDEBAR_LOCK_SET_HINT: &str = "sidebar.lock_set_hint";
    pub const SIDEBAR_LOCK_SET_PASSWORD: &str = "sidebar.lock_set_password";
    pub const SIDEBAR_LOCK_NOW: &str = "sidebar.lock_now";
    pub const SIDEBAR_LOCK_UNLOCK_TITLE: &str = "sidebar.lock_unlock_title";
    pub const SIDEBAR_LOCK_UNLOCK_HINT: &str = "sidebar.lock_unlock_hint";
    pub const SIDEBAR_LOCK_ENTER_PASSWORD: &str = "sidebar.lock_enter_password";
    pub const SIDEBAR_LOCK_UNLOCK: &str = "sidebar.lock_unlock";
    pub const SIDEBAR_LOCK_WRONG_PASSWORD: &str = "sidebar.lock_wrong_password";

    // Connect
    pub const CONNECT_TITLE_LOCAL: &str = "connect.title_local";
    pub const CONNECT_LABEL_LOCAL_ID: &str = "connect.label_local_id";
    pub const CONNECT_LABEL_OTP: &str = "connect.label_otp";
    pub const CONNECT_TITLE_REMOTE: &str = "connect.title_remote";
    pub const CONNECT_PLACEHOLDER_ID: &str = "connect.placeholder_id";
    pub const CONNECT_PW_SEP: &str = "connect.pw_sep";
    pub const CONNECT_BTN_CONNECT: &str = "connect.btn_connect";
    pub const CONNECT_RADIO_DESKTOP: &str = "connect.radio_desktop";
    pub const CONNECT_RADIO_FILE: &str = "connect.radio_file";
    pub const CONNECT_TIPS_TITLE: &str = "connect.tips_title";
    pub const CONNECT_TIPS_L1: &str = "connect.tips_l1";
    pub const CONNECT_TIPS_L2: &str = "connect.tips_l2";
    pub const CONNECT_TIPS_L3: &str = "connect.tips_l3";

    // Home
    pub const HOME_GROUP_MY: &str = "home.group_my";
    pub const HOME_GROUP_REMOTE: &str = "home.group_remote";
    pub const HOME_SEARCH_PLACEHOLDER: &str = "home.search_placeholder";
    pub const HOME_ADD_DEVICE: &str = "home.add_device";
    pub const HOME_BADGE_LOCAL: &str = "home.badge_local";
    pub const HOME_SELECT_HINT: &str = "home.select_hint";
    pub const HOME_DETAIL_TITLE: &str = "home.detail_title";
    pub const HOME_DETAIL_NAME: &str = "home.detail_name";
    pub const HOME_DETAIL_ALIAS: &str = "home.detail_alias";
    pub const HOME_DETAIL_SET_ALIAS: &str = "home.detail_set_alias";
    pub const HOME_DETAIL_UNATTENDED: &str = "home.detail_unattended";
    pub const HOME_DETAIL_SYS_AUTH: &str = "home.detail_sys_auth";
    pub const HOME_DETAIL_SET: &str = "home.detail_set";
    pub const HOME_DETAIL_UNAUTH: &str = "home.detail_unauth";
    pub const HOME_DETAIL_PW_AUTH: &str = "home.detail_pw_auth";
    pub const HOME_DETAIL_ACTIVE: &str = "home.detail_active";
    pub const HOME_DETAIL_AUTOSTART: &str = "home.detail_autostart";
    pub const HOME_DETAIL_NOT_FOUND: &str = "home.detail_not_found";
    pub const HOME_LOCAL_NAME: &str = "home.local_name";

    // Host
    pub const HOST_TITLE: &str = "host.title";
    pub const HOST_SUBTITLE: &str = "host.subtitle";
    pub const HOST_STOPPED: &str = "host.stopped";
    pub const HOST_STARTING: &str = "host.starting";
    pub const HOST_RUNNING: &str = "host.running";
    pub const HOST_ERROR: &str = "host.error";
    pub const HOST_BTN_STOP: &str = "host.btn_stop";
    pub const HOST_BTN_START: &str = "host.btn_start";
    pub const HOST_LABEL_ID: &str = "host.label_id";
    pub const HOST_BTN_COPY: &str = "host.btn_copy";
    pub const HOST_LABEL_PW: &str = "host.label_pw";
    pub const HOST_PW_OPEN: &str = "host.pw_open";
    pub const HOST_SECTION_PW: &str = "host.section_pw";
    pub const HOST_FIELD_PW: &str = "host.field_pw";
    pub const HOST_PLACEHOLDER_PW: &str = "host.placeholder_pw";
    pub const HOST_BTN_RANDOM_TOOLTIP: &str = "host.btn_random_tooltip";
    pub const HOST_BTN_RANDOM: &str = "host.btn_random";
    pub const HOST_BTN_CLEAR: &str = "host.btn_clear";
    pub const HOST_PW_HELP: &str = "host.pw_help";
    pub const HOST_RUNNING_TITLE: &str = "host.running_title";
    pub const HOST_RUNNING_HINT: &str = "host.running_hint";
    pub const HOST_SIGNAL_ADDR: &str = "host.signal_addr";
    pub const HOST_PROCESS_EXITED: &str = "host.process_exited";
    pub const HOST_SERVICE_SECTION: &str = "host.service_section";
    pub const HOST_SERVICE_NOT_INSTALLED: &str = "host.service_not_installed";
    pub const HOST_SERVICE_NOT_INSTALLED_HINT: &str = "host.service_not_installed_hint";
    pub const HOST_SERVICE_INSTALLED: &str = "host.service_installed";
    pub const HOST_SERVICE_INSTALLED_HINT: &str = "host.service_installed_hint";
    pub const HOST_SERVICE_NOT_INSTALLED_HINT_WIN: &str = "host.service_not_installed_hint_win";
    pub const HOST_SERVICE_INSTALLED_HINT_WIN: &str = "host.service_installed_hint_win";
    pub const HOST_BTN_INSTALL_SVC: &str = "host.btn_install_svc";
    pub const HOST_BTN_UNINSTALL_SVC: &str = "host.btn_uninstall_svc";
    pub const HOST_SVC_INSTALLING: &str = "host.svc_installing";
    pub const HOST_SVC_UNINSTALLING: &str = "host.svc_uninstalling";
    pub const HOST_SVC_INSTALL_OK: &str = "host.svc_install_ok";
    pub const HOST_SVC_INSTALL_ERR: &str = "host.svc_install_err";
    pub const HOST_SVC_UNINSTALL_OK: &str = "host.svc_uninstall_ok";
    pub const HOST_SVC_UNINSTALL_ERR: &str = "host.svc_uninstall_err";

    // Settings dialog tabs
    pub const SETTINGS_TAB_GENERAL: &str = "settings.tab_general";
    pub const SETTINGS_TAB_SECURITY: &str = "settings.tab_security";
    pub const SETTINGS_TAB_NETWORK: &str = "settings.tab_network";
    pub const SETTINGS_TAB_DISPLAY: &str = "settings.tab_display";
    pub const SETTINGS_TAB_ACCOUNT: &str = "settings.tab_account";
    pub const SETTINGS_TAB_ABOUT: &str = "settings.tab_about";

    // Settings – General
    pub const SETTINGS_SERVICE: &str = "settings.service";
    pub const SETTINGS_SERVICE_START: &str = "settings.service_start";
    pub const SETTINGS_SERVICE_STOP: &str = "settings.service_stop";
    pub const SETTINGS_THEME: &str = "settings.theme";
    pub const SETTINGS_THEME_LIGHT: &str = "settings.theme_light";
    pub const SETTINGS_THEME_DARK: &str = "settings.theme_dark";
    pub const SETTINGS_THEME_FOLLOW_SYS: &str = "settings.theme_follow_sys";
    pub const SETTINGS_LANG: &str = "settings.lang";
    pub const SETTINGS_HW_CODEC: &str = "settings.hw_codec";
    pub const SETTINGS_HW_CODEC_ENABLE: &str = "settings.hw_codec_enable";
    pub const SETTINGS_OTHER: &str = "settings.other";
    pub const SETTINGS_CONFIRM_CLOSE_TABS: &str = "settings.confirm_close_tabs";
    pub const SETTINGS_AUTOSTART: &str = "settings.autostart";
    pub const SETTINGS_MINIMIZE_TO_TRAY: &str = "settings.minimize_to_tray";

    // Settings – Security
    pub const SETTINGS_SECURITY: &str = "settings.security";
    pub const SETTINGS_PERMISSIONS: &str = "settings.permissions";
    pub const SETTINGS_PERM_KEYBOARD: &str = "settings.perm_keyboard";
    pub const SETTINGS_PERM_CLIPBOARD: &str = "settings.perm_clipboard";
    pub const SETTINGS_PERM_AUDIO: &str = "settings.perm_audio";
    pub const SETTINGS_PERM_FILE: &str = "settings.perm_file";
    pub const SETTINGS_PASSWORD: &str = "settings.password";
    pub const SETTINGS_PASSWORD_PERMANENT: &str = "settings.password_permanent";
    pub const SETTINGS_PASSWORD_OTP: &str = "settings.password_otp";
    pub const SETTINGS_PASSWORD_USE_PERMANENT: &str = "settings.password_use_permanent";
    pub const SETTINGS_ID_SECTION: &str = "settings.id_section";
    pub const SETTINGS_CHANGE_ID: &str = "settings.change_id";
    pub const SETTINGS_LOCK_ON_SESSION_END: &str = "settings.lock_on_session_end";
    pub const SETTINGS_DENY_LAN_DISCOVERY: &str = "settings.deny_lan_discovery";
    pub const SETTINGS_APPROVE_MODE_INTERACTIVE: &str = "settings.approve_mode_interactive";
    // Password access mode
    pub const SETTINGS_ACCESS_MODE: &str = "settings.access_mode";
    pub const SETTINGS_ACCESS_MODE_OTP: &str = "settings.access_mode_otp";
    pub const SETTINGS_ACCESS_MODE_PERMANENT: &str = "settings.access_mode_permanent";
    pub const SETTINGS_ACCESS_MODE_BOTH: &str = "settings.access_mode_both";
    pub const SETTINGS_ACCESS_MODE_CLICK: &str = "settings.access_mode_click";
    pub const SETTINGS_OTP_USE: &str = "settings.otp_use";
    pub const SETTINGS_OTP_LEN: &str = "settings.otp_len";
    pub const SETTINGS_OTP_DIGITS_ONLY: &str = "settings.otp_digits_only";
    pub const SETTINGS_PERM_USE: &str = "settings.perm_use";
    pub const SETTINGS_PERM_SET_BTN: &str = "settings.perm_set_btn";
    pub const SETTINGS_BOTH_USE: &str = "settings.both_use";
    // Set permanent password dialog
    pub const SETTINGS_SET_PERM_PW_TITLE: &str = "settings.set_perm_pw_title";
    pub const SETTINGS_SET_PERM_PW_HINT: &str = "settings.set_perm_pw_hint";
    pub const SETTINGS_SET_PERM_PW_NEW: &str = "settings.set_perm_pw_new";
    pub const SETTINGS_SET_PERM_PW_CONFIRM: &str = "settings.set_perm_pw_confirm";
    pub const SETTINGS_SET_PERM_PW_MISMATCH: &str = "settings.set_perm_pw_mismatch";
    pub const SETTINGS_SET_PERM_PW_EMPTY: &str = "settings.set_perm_pw_empty";
    pub const SETTINGS_SET_PERM_PW_SAVE: &str = "settings.set_perm_pw_save";
    pub const SETTINGS_SET_PERM_PW_CLEAR: &str = "settings.set_perm_pw_clear";
    pub const SETTINGS_SET_PERM_PW_SAVED: &str = "settings.set_perm_pw_saved";

    // Settings – Network
    pub const SETTINGS_NETWORK: &str = "settings.network";
    pub const SETTINGS_ID_RELAY_SERVER: &str = "settings.id_relay_server";
    pub const SETTINGS_SIGNAL_ADDR: &str = "settings.signal_addr";
    pub const SETTINGS_SIGNAL_HINT: &str = "settings.signal_hint";
    pub const SETTINGS_PLACEHOLDER_HOST: &str = "settings.placeholder_host";
    pub const SETTINGS_RELAY_SERVER: &str = "settings.relay_server";
    pub const SETTINGS_RELAY_SERVER_HINT: &str = "settings.relay_server_hint";
    pub const SETTINGS_PROXY: &str = "settings.proxy";
    pub const SETTINGS_PROXY_HOST: &str = "settings.proxy_host";
    pub const SETTINGS_PROXY_HINT: &str = "settings.proxy_hint";
    pub const SETTINGS_STUN: &str = "settings.stun";
    pub const SETTINGS_STUN_HINT: &str = "settings.stun_hint";
    pub const SETTINGS_PLACEHOLDER_STUN: &str = "settings.placeholder_stun";
    pub const SETTINGS_STUN_HELP: &str = "settings.stun_help";
    pub const SETTINGS_BANDWIDTH: &str = "settings.bandwidth";
    pub const SETTINGS_BANDWIDTH_KBPS: &str = "settings.bandwidth_kbps";
    pub const SETTINGS_BANDWIDTH_HINT: &str = "settings.bandwidth_hint";

    // Settings – Display
    pub const SETTINGS_DISPLAY: &str = "settings.display";
    pub const SETTINGS_VIEW_STYLE: &str = "settings.view_style";
    pub const SETTINGS_VIEW_ORIGINAL: &str = "settings.view_original";
    pub const SETTINGS_VIEW_ADAPTIVE: &str = "settings.view_adaptive";
    pub const SETTINGS_SCROLL_STYLE: &str = "settings.scroll_style";
    pub const SETTINGS_SCROLL_AUTO: &str = "settings.scroll_auto";
    pub const SETTINGS_SCROLL_BAR: &str = "settings.scroll_bar";
    pub const SETTINGS_IMAGE_QUALITY: &str = "settings.image_quality";
    pub const SETTINGS_QUALITY_BEST: &str = "settings.quality_best";
    pub const SETTINGS_QUALITY_BALANCED: &str = "settings.quality_balanced";
    pub const SETTINGS_QUALITY_LOW: &str = "settings.quality_low";
    pub const SETTINGS_QUALITY_CUSTOM: &str = "settings.quality_custom";
    pub const SETTINGS_CODEC: &str = "settings.codec";
    pub const SETTINGS_CODEC_AUTO: &str = "settings.codec_auto";
    pub const SETTINGS_CODEC_VP9: &str = "settings.codec_vp9";
    pub const SETTINGS_CODEC_H264: &str = "settings.codec_h264";
    pub const SETTINGS_CODEC_H265: &str = "settings.codec_h265";
    pub const SETTINGS_DISPLAY_OTHER: &str = "settings.display_other";
    pub const SETTINGS_SYNC_CLIPBOARD: &str = "settings.sync_clipboard";
    pub const SETTINGS_SHOW_REMOTE_CURSOR: &str = "settings.show_remote_cursor";
    pub const SETTINGS_SHOW_MONITORS_TOOLBAR: &str = "settings.show_monitors_toolbar";
    pub const SETTINGS_SHOW_MAGNIFIER: &str = "settings.show_magnifier";
    pub const SETTINGS_DISABLE_ANIMATIONS: &str = "settings.disable_animations";

    // Settings – Account
    pub const SETTINGS_ACCOUNT: &str = "settings.account";
    pub const SETTINGS_BTN_LOGIN: &str = "settings.btn_login";
    pub const SETTINGS_BTN_LOGOUT: &str = "settings.btn_logout";

    // Settings – About
    pub const SETTINGS_ABOUT: &str = "settings.about";
    pub const SETTINGS_ABOUT_TITLE: &str = "settings.about_title";
    pub const SETTINGS_ABOUT_VER: &str = "settings.about_ver";
    pub const SETTINGS_ABOUT_BUILD_DATE: &str = "settings.about_build_date";
    pub const SETTINGS_ABOUT_FINGERPRINT: &str = "settings.about_fingerprint";
    pub const SETTINGS_ABOUT_PRIVACY: &str = "settings.about_privacy";
    pub const SETTINGS_ABOUT_WEBSITE: &str = "settings.about_website";
    pub const SETTINGS_ABOUT_COPYRIGHT: &str = "settings.about_copyright";

    // Shared
    pub const SETTINGS_TITLE: &str = "settings.title";
    pub const SETTINGS_DEVICE_ID: &str = "settings.device_id";
    pub const SETTINGS_LOCAL_ID: &str = "settings.local_id";
    pub const SETTINGS_LOCAL_ID_HINT: &str = "settings.local_id_hint";
    pub const SETTINGS_BTN_RESET_TOOLTIP: &str = "settings.btn_reset_tooltip";
    pub const SETTINGS_BTN_RESET: &str = "settings.btn_reset";
    pub const SETTINGS_QUALITY: &str = "settings.quality";
    pub const SETTINGS_QUALITY_HINT: &str = "settings.quality_hint";
    pub const SETTINGS_LANG_HINT: &str = "settings.lang_hint";
    pub const SETTINGS_THEME_HINT: &str = "settings.theme_hint";
    pub const SETTINGS_THEME_SKY_BLUE: &str = "settings.theme_sky_blue";
    pub const SETTINGS_THEME_GRASS_GREEN: &str = "settings.theme_grass_green";
    pub const SETTINGS_THEME_ORANGE: &str = "settings.theme_orange";
    pub const SETTINGS_THEME_PURPLE: &str = "settings.theme_purple";

    // Auth
    pub const AUTH_ACCOUNT: &str = "auth.account";
    pub const AUTH_NOT_LOGGED_IN: &str = "auth.not_logged_in";
    pub const AUTH_LOGGING_IN: &str = "auth.logging_in";
    pub const AUTH_REGISTERING: &str = "auth.registering";
    pub const AUTH_BTN_LOGOUT: &str = "auth.btn_logout";
    pub const AUTH_LABEL_USER: &str = "auth.label_user";
    pub const AUTH_PLACEHOLDER_USER: &str = "auth.placeholder_user";
    pub const AUTH_LABEL_PW: &str = "auth.label_pw";
    pub const AUTH_PLACEHOLDER_PW: &str = "auth.placeholder_pw";
    pub const AUTH_BTN_LOGIN: &str = "auth.btn_login";
    pub const AUTH_BTN_REGISTER: &str = "auth.btn_register";
    pub const AUTH_HELP: &str = "auth.help";
    pub const AUTH_NO_SERVER: &str = "auth.no_server";
    pub const AUTH_CONN_FAILED: &str = "auth.conn_failed";
    pub const AUTH_UNKNOWN_OP: &str = "auth.unknown_op";
    pub const AUTH_SERIALIZE_FAILED: &str = "auth.serialize_failed";
    pub const AUTH_SEND_FAILED: &str = "auth.send_failed";
    pub const AUTH_TIMEOUT: &str = "auth.timeout";
    pub const AUTH_DISCONNECTED: &str = "auth.disconnected";
    pub const AUTH_PARSE_FAILED: &str = "auth.parse_failed";
    pub const AUTH_UNEXPECTED_RESP: &str = "auth.unexpected_resp";

    // Remote / Viewer
    pub const REMOTE_CONNECTING: &str = "remote.connecting";
    pub const REMOTE_ESTABLISHING: &str = "remote.establishing";
    pub const REMOTE_VIEW_ONLY: &str = "remote.view_only";
    pub const REMOTE_CONNECTED: &str = "remote.connected";
    pub const REMOTE_STATS: &str = "remote.stats";
    pub const REMOTE_DISCONNECTED: &str = "remote.disconnected";
    pub const REMOTE_CONN_FAILED: &str = "remote.conn_failed";
    pub const REMOTE_SCREENSHOT_SAVED: &str = "remote.screenshot_saved";
    pub const REMOTE_SCREENSHOT_FAILED: &str = "remote.screenshot_failed";
    pub const REMOTE_FILE_TOO_LARGE: &str = "remote.file_too_large";
    pub const REMOTE_FILE_LIST_FAILED: &str = "remote.file_list_failed";
    pub const REMOTE_FILE_READ_FAILED: &str = "remote.file_read_failed";
    pub const REMOTE_FILE_SEND_FAILED: &str = "remote.file_send_failed";
    pub const REMOTE_ERR_TITLE: &str = "remote.err_title";
    pub const REMOTE_ERR_BACK: &str = "remote.err_back";

    // Toolbar
    pub const TB_UNPIN: &str = "tb.unpin";
    pub const TB_PIN: &str = "tb.pin";
    pub const TB_QUALITY: &str = "tb.quality";
    pub const TB_SCALE: &str = "tb.scale";
    pub const TB_EXIT_VIEW_ONLY: &str = "tb.exit_view_only";
    pub const TB_VIEW_ONLY: &str = "tb.view_only";
    pub const TB_SCREENSHOT: &str = "tb.screenshot";
    pub const TB_SEND_FILE: &str = "tb.send_file";
    pub const TB_CLIPBOARD_SYNCED: &str = "tb.clipboard_synced";
    pub const TB_SYNC_CLIPBOARD: &str = "tb.sync_clipboard";
    pub const TB_MINIMIZE: &str = "tb.minimize";
    pub const TB_EXIT_FULLSCREEN: &str = "tb.exit_fullscreen";
    pub const TB_FULLSCREEN: &str = "tb.fullscreen";
    pub const TB_DISCONNECT: &str = "tb.disconnect";

    // File Transfer
    pub const FILE_TRANSFERRING: &str = "file.transferring";
    pub const FILE_DROP: &str = "file.drop";

    // Quality / Scale
    pub const QUALITY_BEST: &str = "quality.best";
    pub const QUALITY_BALANCED: &str = "quality.balanced";
    pub const QUALITY_SMOOTH: &str = "quality.smooth";
    pub const QUALITY_LOW_BW: &str = "quality.low_bw";
    pub const SCALE_FIT: &str = "scale.fit";
    pub const SCALE_STRETCH: &str = "scale.stretch";
    pub const SCALE_ORIGINAL: &str = "scale.original";

    // Generic
    pub const GEN_COPY: &str = "gen.copy";
    pub const GEN_COPIED: &str = "gen.copied";
    pub const GEN_INPUT_HOSTPORT: &str = "settings.placeholder_host";
}
