//! Application configuration: persistence, loading, and saving.

use serde::{Deserialize, Serialize};

pub use dioxus_bootstrap_css::theme::Theme;

// ─── Serde-compatible theme wrapper ────────────────────────────────────────

/// Wraps `Theme` with Serialize/Deserialize for config persistence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeConfig(pub Theme);

impl Default for ThemeConfig {
    fn default() -> Self { ThemeConfig(Theme::Light) }
}

impl Serialize for ThemeConfig {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for ThemeConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(ThemeConfig(match s.as_str() {
            "dark" | "Dark" => Theme::Dark,
            _ => Theme::Light,
        }))
    }
}

// ─── History entry ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub remote_id: String,
    pub server_addr: String,
    /// Human-readable alias (optional, for display).
    #[serde(default)]
    pub alias: String,
    /// Unix timestamp of the last successful connection.
    #[serde(default)]
    pub last_connected: u64,
}

// ─── Config struct ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub server_addr: String,
    #[serde(default)]
    pub remote_id: String,
    /// Locally generated unique device ID (for signaling registration).
    #[serde(default)]
    pub local_id: String,
    /// STUN server list.  Empty → use default.
    #[serde(default)]
    pub stun_servers: Vec<String>,
    /// UI theme preference ("dark" or "light").
    #[serde(default)]
    pub theme: ThemeConfig,
    /// UI language / locale id (BCP 47 tag, from `i18n.locale` in *.ini).
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Host access password (empty = open access).
    #[serde(default)]
    pub host_password: String,
    /// Connection history (most recent first).
    #[serde(default)]
    pub connection_history: Vec<HistoryEntry>,
    /// Login username for signaling server authentication.
    #[serde(default)]
    pub login_username: String,
    /// Login password for signaling server authentication.
    #[serde(default)]
    pub login_password: String,
    /// Display quality preset index (index into QUALITY_PRESETS).
    #[serde(default = "default_quality_idx")]
    pub quality: usize,
}

fn default_locale() -> String {
    crate::i18n::default_locale_id().to_string()
}

fn default_quality_idx() -> usize {
    shared::frame::DEFAULT_QUALITY_IDX
}

// ─── Config path ────────────────────────────────────────────────────────────

pub fn config_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    return std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        });

    #[cfg(target_os = "macos")]
    return std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join("Library")
            .join("Application Support")
    });

    #[cfg(target_os = "windows")]
    return std::env::var("APPDATA").ok().map(std::path::PathBuf::from);

    #[allow(unreachable_code)]
    None
}

pub fn config_path() -> std::path::PathBuf {
    config_dir()
        .map(|d| d.join("rtcdesk"))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rtcdesk.json")
}

// ─── Load / save ────────────────────────────────────────────────────────────

/// Load config from disk.  Automatically generates and persists a local_id
/// if it is missing.
pub fn load_config() -> AppConfig {
    let path = config_path();
    let mut cfg: AppConfig = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    };
    if cfg.local_id.is_empty() {
        cfg.local_id = crate::local_id::generate_local_id();
        save_config(&cfg);
    }
    cfg
}

pub fn save_config(cfg: &AppConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, json);
    }
}

/// Add or update a history entry.  If the same (remote_id, server_addr) pair
/// already exists, its `last_connected` timestamp is updated; otherwise a new
/// entry is prepended.  The list is capped at 50 entries.
pub fn add_history_entry(cfg: &mut AppConfig, remote_id: &str, server_addr: &str, alias: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if let Some(existing) = cfg.connection_history.iter_mut().find(|e| {
        e.remote_id == remote_id && e.server_addr == server_addr
    }) {
        existing.last_connected = now;
        if !alias.is_empty() {
            existing.alias = alias.to_string();
        }
    } else {
        cfg.connection_history.insert(0, HistoryEntry {
            remote_id: remote_id.to_string(),
            server_addr: server_addr.to_string(),
            alias: alias.to_string(),
            last_connected: now,
        });
    }

    // Cap at 50 entries
    cfg.connection_history.truncate(50);
    save_config(cfg);
}

// ─── STUN server list ────────────────────────────────────────────────────────

/// Priority: config file → RTCDESK_STUN env var → default Google STUN.
pub fn load_stun_servers() -> Vec<String> {
    let cfg = load_config();
    if !cfg.stun_servers.is_empty() {
        return cfg.stun_servers;
    }
    let raw = std::env::var("RTCDESK_STUN").unwrap_or_default();
    let v: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if v.is_empty() {
        vec![shared::frame::DEFAULT_STUN.into()]
    } else {
        v
    }
}
