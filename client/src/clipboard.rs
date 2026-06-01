//! System clipboard helpers — backed by `clipboard-rs`.
//!
//! Supports Text, File-list (file:// URI list) and Image (RGBA raw data)
//! on all three platforms (Windows / macOS / Linux X11+Wayland).
//!
//! `clipboard-rs` handles the platform specifics, so this module is free
//! of external process spawning (no xclip / wl-paste / wl-copy).

use clipboard_rs::{Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher,
                   ClipboardWatcherContext, WatcherShutdown, common::RustImage, RustImageData};
use log::{error, warn};

// ── Clipboard context factory ────────────────────────────────────────────────

/// Create a new OS clipboard context.  Returns an error string on failure.
fn ctx() -> Result<ClipboardContext, String> {
    ClipboardContext::new().map_err(|e| format!("clipboard context: {e}"))
}

// ── Text ─────────────────────────────────────────────────────────────────────

/// Read plain text from the system clipboard.
pub fn get_system_clipboard() -> Result<String, String> {
    ctx()?.get_text().map_err(|e| format!("{e}"))
}

/// Write plain text to the system clipboard.
pub fn set_system_clipboard(text: &str) -> Result<(), String> {
    ctx()?.set_text(text.to_owned()).map_err(|e| format!("{e}"))
}

// ── File URIs ────────────────────────────────────────────────────────────────

/// Read `file://` URIs from the system clipboard.
///
/// Returns `None` when the clipboard does not hold a file list.
pub fn get_clipboard_file_uris() -> Option<String> {
    let c = ctx().ok()?;
    let files = c.get_files().ok()?;
    if files.is_empty() {
        return None;
    }
    // Normalise to `file://` URI list (one per line)
    let uris: Vec<String> = files
        .into_iter()
        .map(|p| {
            // clipboard-rs returns plain paths on some platforms; ensure file:// prefix
            if p.starts_with("file://") {
                p
            } else {
                format!("file://{p}")
            }
        })
        .collect();
    Some(uris.join("\n"))
}

/// Write a newline-separated `file://` URI list to the system clipboard.
pub fn set_clipboard_file_uris(uris: &str) -> Result<(), String> {
    let paths: Vec<String> = uris
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            // Strip file:// prefix — clipboard-rs expects plain paths
            if let Some(p) = l.strip_prefix("file://") {
                Some(p.to_owned())
            } else {
                Some(l.to_owned())
            }
        })
        .collect();

    if paths.is_empty() {
        return Ok(());
    }

    ctx()?
        .set_files(paths)
        .map_err(|e| format!("set_files: {e}"))
}

// ── Image ────────────────────────────────────────────────────────────────────

/// Clipboard image data returned from [`get_clipboard_image`].
pub struct ClipboardImage {
    /// Raw RGBA pixels, row-major.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Read an image from the system clipboard.
///
/// Returns `None` when no image is present or an error occurs.
pub fn get_clipboard_image() -> Option<ClipboardImage> {
    let c = ctx().ok()?;
    let img = c.get_image().ok()?;
    let (width, height) = img.get_size();
    let rgba = img.to_rgba8().ok()?;
    Some(ClipboardImage {
        rgba: rgba.to_vec(),
        width: width as u32,
        height: height as u32,
    })
}

/// Write raw RGBA image data to the system clipboard.
pub fn set_clipboard_image(rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let img = RustImageData::from_bytes(rgba)
    .map_err(|e| format!("from_rgba: {e}"))?;
    ctx()?
        .set_image(img)
        .map_err(|e| format!("set_image: {e}"))
}

// ── URI helpers ───────────────────────────────────────────────────────────────

/// Parse a newline-separated `file://` URI string into local `PathBuf`s.
pub fn parse_file_uris(uris: &str) -> Vec<std::path::PathBuf> {
    uris.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("file://") {
                Some(std::path::PathBuf::from(&line[7..]))
            } else if !line.is_empty() {
                Some(std::path::PathBuf::from(line))
            } else {
                None
            }
        })
        .collect()
}
