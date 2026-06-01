//! Remote session toolbar: floating, auto-hiding, quality/scale menus.

use dioxus::prelude::*;
use crate::i18n::{t as tr};
use crate::i18n::keys;
use crate::quality::{ScaleMode, QUALITY_PRESETS, SCALE_MODES};
use crate::clipboard::get_system_clipboard;

// ─── Toolbar component ────────────────────────────────────────────────────

#[component]
pub fn RemoteToolbar(
    toolbar_visible:      Signal<bool>,
    toolbar_pinned:       Signal<bool>,
    scale_mode:           Signal<ScaleMode>,
    quality_idx:          Signal<usize>,
    clipboard_synced:     Signal<bool>,
    is_fullscreen:        Signal<bool>,
    show_quality_menu:    Signal<bool>,
    show_scale_menu:      Signal<bool>,
    view_only:            Signal<bool>,
    q_name:               String,
    locale:               Signal<String>,
    on_show_quality_menu: EventHandler<()>,
    on_show_scale_menu:   EventHandler<()>,
    on_select_quality:    EventHandler<usize>,
    on_select_scale:      EventHandler<ScaleMode>,
    on_toggle_clipboard:  EventHandler<()>,
    on_disconnect:        EventHandler<()>,
    on_toggle_fullscreen: EventHandler<()>,
    on_minimize:          EventHandler<()>,
    on_toggle_pin:        EventHandler<()>,
    on_toggle_view_only:  EventHandler<()>,
    on_screenshot:        EventHandler<()>,
    on_open_file_dialog:  EventHandler<()>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let ty = if toolbar_visible() || toolbar_pinned() {
        "0px"
    } else {
        "-40px"
    };
    rsx! {
        div {
            class: "remote-toolbar",
            style: "transform: translateX(-50%) translateY({ty});",
            onmouseenter: move |_| { if !toolbar_pinned() { toolbar_visible.set(true); } },
            onmouseleave: move |_| { if !toolbar_pinned() { toolbar_visible.set(false); } },

            {tb_icon_btn(
                if toolbar_pinned() { "📍" } else { "📌" },
                if toolbar_pinned() { tr(keys::TB_UNPIN, l) } else { tr(keys::TB_PIN, l) },
                if toolbar_pinned() { "var(--accent)" } else { "var(--text-sec)" },
                on_toggle_pin,
            )}
            div { class: "remote-toolbar-divider" }

            // Quality menu
            div { style: "position: relative;",
                {tb_btn_text(tr(keys::TB_QUALITY, l), &q_name, false, on_show_quality_menu)}
                if show_quality_menu() {
                    div {
                        class: "tb-dropdown",
                        onmouseleave: move |_| show_quality_menu.set(false),
                        for (idx, preset) in QUALITY_PRESETS.iter().enumerate() {
                            {
                                let a = idx == quality_idx();
                                let (clr, bg) = if a {
                                    ("var(--accent)", "var(--accent-bg)")
                                } else {
                                    ("var(--text-pri)", "transparent")
                                };
                                let ql = crate::i18n::quality_label(idx, l);
                                rsx! {
                                    div { key: "q_{idx}",
                                        class: "tb-dropdown-item",
                                        style: "color: {clr}; background: {bg};",
                                        onmousedown: move |_| on_select_quality.call(idx),
                                        span { "{ql}" }
                                        span { class: "text-dim", style: "font-size: 11px;", "{preset.2}x{preset.3}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "remote-toolbar-divider" }

            // Scale menu
            div { style: "position: relative;",
                {tb_btn_text(tr(keys::TB_SCALE, l), crate::i18n::scale_label(scale_mode(), l), false, on_show_scale_menu)}
                if show_scale_menu() {
                    div {
                        class: "tb-dropdown-sm",
                        onmouseleave: move |_| show_scale_menu.set(false),
                        for sm in SCALE_MODES {
                            {
                                let a = *sm == scale_mode();
                                let (clr, bg) = if a {
                                    ("var(--accent)", "var(--accent-bg)")
                                } else {
                                    ("var(--text-pri)", "transparent")
                                };
                                let s = *sm;
                                let sl = crate::i18n::scale_label(s, l);
                                rsx! {
                                    div { key: "sm_{s:?}",
                                        class: "tb-dropdown-item",
                                        style: "color: {clr}; background: {bg};",
                                        onmousedown: move |_| on_select_scale.call(s),
                                        "{sl}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "remote-toolbar-divider" }

            {tb_icon_btn(
                if view_only() { "👁" } else { "👁‍🗨" },
                if view_only() { tr(keys::TB_EXIT_VIEW_ONLY, l) } else { tr(keys::TB_VIEW_ONLY, l) },
                if view_only() { "var(--warn)" } else { "var(--text-pri)" },
                on_toggle_view_only,
            )}
            {tb_icon_btn("📷", tr(keys::TB_SCREENSHOT, l), "var(--text-pri)", on_screenshot)}
            {tb_icon_btn("📎", tr(keys::TB_SEND_FILE, l), "var(--text-pri)", on_open_file_dialog)}

            div { class: "remote-toolbar-divider" }

            {tb_icon_btn(
                if clipboard_synced() { "✔" } else { "📋" },
                if clipboard_synced() { tr(keys::TB_CLIPBOARD_SYNCED, l) } else { tr(keys::TB_SYNC_CLIPBOARD, l) },
                if clipboard_synced() { "var(--accent)" } else { "var(--text-pri)" },
                on_toggle_clipboard,
            )}

            div { class: "remote-toolbar-divider" }

            {tb_icon_btn("━", tr(keys::TB_MINIMIZE, l), "var(--text-pri)", on_minimize)}

            {tb_icon_btn(
                if is_fullscreen() { "⊡" } else { "⛶" },
                if is_fullscreen() { tr(keys::TB_EXIT_FULLSCREEN, l) } else { tr(keys::TB_FULLSCREEN, l) },
                "var(--text-pri)",
                on_toggle_fullscreen,
            )}

            {tb_icon_btn_danger("✕", tr(keys::TB_DISCONNECT, l), on_disconnect)}
        }
    }
}

// ─── Toolbar helper renderers ─────────────────────────────────────────────

fn tb_icon_btn(
    icon: &str,
    hint: &str,
    color: &str,
    onclick: EventHandler<()>,
) -> Element {
    let color_owned = color.to_string();
    rsx! {
        div {
            class: "tb-icon-btn",
            style: "color: {color_owned};",
            title: hint,
            onclick: move |_| onclick.call(()),
            "{icon}"
        }
    }
}

fn tb_icon_btn_danger(icon: &str, hint: &str, onclick: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "tb-icon-btn",
            style: "color: var(--danger);",
            title: hint,
            onclick: move |_| onclick.call(()),
            "{icon}"
        }
    }
}

fn tb_btn_text(
    label: &str,
    value: &str,
    active: bool,
    onclick: EventHandler<()>,
) -> Element {
    let clr = if active { "var(--accent)" } else { "var(--text-pri)" };
    rsx! {
        div {
            class: "tb-text-btn",
            style: "color: {clr};",
            onclick: move |_| onclick.call(()),
            span { style: "color: var(--text-sec);", "{label}:" }
            span { style: "font-weight: 500;", "{value}" }
        }
    }
}

// ─── Clipboard toolbar action ─────────────────────────────────────────────

pub fn handle_clipboard_toggle(
    mut local_clipboard: Signal<String>,
    mut clipboard_synced: Signal<bool>,
    viewer_clipboard_tx: Signal<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
) {
    if let Ok(text) = get_system_clipboard() {
        if !text.is_empty() && text != local_clipboard() {
            local_clipboard.set(text.clone());
            if let Some(tx) = viewer_clipboard_tx() {
                let _ = tx.send(text);
            }
            clipboard_synced.set(true);
        }
    }
    clipboard_synced.set(!clipboard_synced());
}

/// 画质变更辅助函数。
pub fn handle_quality_select(
    idx: usize,
    mut quality_idx: Signal<usize>,
    mut show_quality_menu: Signal<bool>,
    viewer_quality_tx: Signal<Option<tokio::sync::mpsc::UnboundedSender<usize>>>,
) {
    quality_idx.set(idx);
    show_quality_menu.set(false);
    if let Some(tx) = viewer_quality_tx() {
        let _ = tx.send(idx);
    }
}
