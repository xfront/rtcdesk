//! Toast 通知组件。

use dioxus::prelude::*;
use std::time::{Duration, Instant};

// ─── Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ToastType {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone)]
pub struct ToastMessage {
    pub text: String,
    pub toast_type: ToastType,
    pub created_at: Instant,
}

impl ToastMessage {
    pub fn success(text: impl Into<String>) -> Self {
        Self { text: text.into(), toast_type: ToastType::Success, created_at: Instant::now() }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), toast_type: ToastType::Error, created_at: Instant::now() }
    }
    pub fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), toast_type: ToastType::Info, created_at: Instant::now() }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > Duration::from_secs(3)
    }
}

// ─── ToastOverlay component ────────────────────────────────────────────

#[component]
pub fn ToastOverlay(toast: Signal<Option<ToastMessage>>) -> Element {
    // 3 秒后自动隐藏
    use_future(move || {
        let mut t = toast;
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Some(ref msg) = t() {
                    if msg.is_expired() {
                        t.set(None);
                    }
                }
            }
        }
    });

    let msg = toast();
    match msg {
        None => rsx! {},
        Some(ref m) => {
            let (icon, toast_class) = match m.toast_type {
                ToastType::Success => ("✓", "toast toast-success"),
                ToastType::Error   => ("✗", "toast toast-error"),
                ToastType::Info    => ("ℹ", "toast toast-info"),
            };
            let text = m.text.clone();
            rsx! {
                div {
                    class: "{toast_class}",
                    span { style: "font-size: 16px;", "{icon}" }
                    "{text}"
                }
            }
        }
    }
}
