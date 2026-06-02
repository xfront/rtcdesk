//! File transfer UI: drag-drop overlay, progress bar, state management.
//! Refactored with dioxus-bootstrap-css components.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

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
                    class: "position-absolute",
                    style: "bottom: 50px; right: 12px; background: rgba(15,17,22,0.9);
                            border: 1px solid #30363d; border-radius: 10px;
                            padding: 12px 16px; min-width: 240px; z-index: 60;
                            backdrop-filter: blur(8px);",
                    div {
                        style: "font-size: 12px; margin-bottom: 6px; color: var(--bs-secondary-color);",
                        "{label}"
                    }
                    Progress {
                        ProgressBar {
                            value: progress * 100.0,
                            animated: true,
                            striped: true,
                        }
                    }
                    div {
                        style: "font-size: 11px; margin-top: 4px; text-align: right; color: var(--bs-tertiary-color);",
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
            class: "position-absolute d-flex align-items-center justify-content-center",
            style: "top: 0; left: 0; right: 0; bottom: 0;
                    background: rgba(56,139,253,0.08);
                    border: 2px dashed rgba(56,139,253,0.5);
                    border-radius: 8px; z-index: 55; pointer-events: none;",
            div {
                style: "font-size: 16px; color: #388bfd;
                        background: rgba(15,17,22,0.8);
                        padding: 16px 28px; border-radius: 10px;",
                "{tr(keys::FILE_DROP, l)}"
            }
        }
    }
}
