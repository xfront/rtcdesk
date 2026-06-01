//! File transfer UI: drag-drop overlay, progress bar, state management.

use dioxus::prelude::*;

use crate::i18n::{t as tr, tf};
use crate::i18n::keys;

// ─── FileTransferState ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum FileTransferState {
    Idle,
    Transferring {
        files: Vec<String>,
        current_idx: usize,
        progress: f64, // 0.0..1.0
    },
    Completed {
        count: usize,
    },
    Error {
        message: String,
    },
}

// ─── FileTransferProgress ───────────────────────────────────────────────

#[component]
pub fn FileTransferProgress(state: Signal<FileTransferState>, locale: Signal<String>) -> Element {
    let s = state();
    let l: &'static str = Box::leak(locale().into_boxed_str());
    match s {
        FileTransferState::Transferring { files, current_idx, progress } => {
            let pct = (progress * 100.0) as u32;
            let current_file = files.get(current_idx).cloned().unwrap_or_default();
            let label = tf(keys::FILE_TRANSFERRING, l, &[("file", &current_file)]);
            rsx! {
                div {
                    class: "file-transfer-panel",
                    div {
                        class: "text-sec",
                        style: "font-size: 12px; margin-bottom: 6px;",
                        "{label}"
                    }
                    div {
                        class: "progress-bar-track",
                        div {
                            class: "progress-bar-fill",
                            style: "width: {pct}%;",
                        }
                    }
                    div {
                        class: "text-dim",
                        style: "font-size: 11px; margin-top: 4px; text-align: right;",
                        "{pct}%"
                    }
                }
            }
        }
        _ => rsx! {},
    }
}

// ─── DropOverlay ────────────────────────────────────────────────────────

#[component]
pub fn DropOverlay(is_dragging: Signal<bool>, locale: Signal<String>) -> Element {
    if !is_dragging() {
        return rsx! {};
    }
    let l: &'static str = Box::leak(locale().into_boxed_str());
    rsx! {
        div {
            class: "drop-overlay",
            div {
                class: "drop-overlay-label",
                "{tr(keys::FILE_DROP, l)}"
            }
        }
    }
}
