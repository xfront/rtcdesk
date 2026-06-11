//! UI component modules for the main window.

use dioxus::core::Element;
use dioxus::core_macro::{component, rsx};

pub mod auth;
pub mod connect;
pub mod home;
pub mod host;
pub mod resize;
pub mod settings_dialog;
pub mod sidebar;
pub mod titlebar;
pub mod icon;
pub mod lock_ui;

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;

use crate::config::{load_config, save_config};
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use dioxus_bootstrap_css::theme::Theme;

#[component]
pub fn SettingsCard(
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