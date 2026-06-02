//! Remote session toolbar: floating, auto-hiding, quality/scale menus.

use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
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
        "-44px"
    };
    rsx! {
        div {
            class: "position-absolute d-flex align-items-center",
            style: "top: 0; left: 50%; transform: translateX(-50%) translateY({ty});
                    background: var(--toolbar); border-radius: 0 0 12px 12px;
                    padding: 5px 10px; gap: 2px; z-index: 50;
                    transition: transform 0.8s ease;
                    box-shadow: 0 4px 20px rgba(0,0,0,0.4);
                    backdrop-filter: blur(12px);
                    border: 1px solid var(--toolbar-border); border-top: none;",
            onmouseenter: move |_| { if !toolbar_pinned() { toolbar_visible.set(true); } },
            onmouseleave: move |_| { if !toolbar_pinned() { toolbar_visible.set(false); } },

            ButtonToolbar {
                ButtonGroup {
                    {tb_icon_btn(
                        if toolbar_pinned() { "📍" } else { "📌" },
                        if toolbar_pinned() { tr(keys::TB_UNPIN, l) } else { tr(keys::TB_PIN, l) },
                        if toolbar_pinned() { "var(--bs-success)" } else { "var(--bs-secondary-color)" },
                        on_toggle_pin,
                    )}
                }

                ButtonGroup {
                    Dropdown {
                        toggle_class: "tb-button",
                        open: show_quality_menu,
                        toggle: rsx! {"🎚"},
                        menu: rsx! {
                            div {
                                class: "tb-dropdown-menu",
                                for (idx, preset) in QUALITY_PRESETS.iter().enumerate() {
                                    {
                                        let a = idx == quality_idx();
                                        let ql = crate::i18n::quality_label(idx, l);
                                        rsx! {
                                            DropdownItem {
                                                key: "q_{idx}",
                                                active: a,
                                                onclick: move |_| on_select_quality.call(idx),
                                                div { class: "small",
                                                    span { "{ql}" }
                                                    span { class: "small", style: "color: var(--bs-tertiary-color);", "{preset.2}x{preset.3}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }

                    // Scale menu
                    Dropdown {
                        toggle_class: "tb-button",
                        open: show_scale_menu,
                        toggle: rsx! {"⊠"},
                        menu: rsx! {
                            div {
                                class: "tb-dropdown-menu",
                                for sm in SCALE_MODES {
                                    {
                                        let a = *sm == scale_mode();
                                        let s = *sm;
                                        let sl = crate::i18n::scale_label(s, l);
                                        rsx! {
                                            DropdownItem {
                                                key: "sm_{s:?}",
                                                active: a,
                                                onclick: move |_| on_select_scale.call(s),
                                                span { class: "small", "{sl}" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }

                ButtonGroup {
                    {tb_icon_btn(
                        if view_only() { "👁" } else { "👁‍🗨" },
                        if view_only() { tr(keys::TB_EXIT_VIEW_ONLY, l) } else { tr(keys::TB_VIEW_ONLY, l) },
                        if view_only() { "var(--bs-warning)" } else { "var(--bs-body-color)" },
                        on_toggle_view_only,
                    )}
                    {tb_icon_btn("📷", tr(keys::TB_SCREENSHOT, l), "var(--bs-body-color)", on_screenshot)}
                    {tb_icon_btn("📎", tr(keys::TB_SEND_FILE, l), "var(--bs-body-color)", on_open_file_dialog)}
                }

                ButtonGroup {
                    {tb_icon_btn(
                        if clipboard_synced() { "✔" } else { "📋" },
                        if clipboard_synced() { tr(keys::TB_CLIPBOARD_SYNCED, l) } else { tr(keys::TB_SYNC_CLIPBOARD, l) },
                        if clipboard_synced() { "var(--bs-success)" } else { "var(--bs-body-color)" },
                        on_toggle_clipboard,
                    )}
                }

                ButtonGroup {
                    {tb_icon_btn("━", tr(keys::TB_MINIMIZE, l), "var(--bs-body-color)", on_minimize)}
                    {tb_icon_btn(
                        if is_fullscreen() { "⊡" } else { "⛶" },
                        if is_fullscreen() { tr(keys::TB_EXIT_FULLSCREEN, l) } else { tr(keys::TB_FULLSCREEN, l) },
                        "var(--bs-body-color)",
                        on_toggle_fullscreen,
                    )}
                }

                ButtonGroup {
                    {tb_icon_btn_danger("✕", tr(keys::TB_DISCONNECT, l), on_disconnect)}
                }
            }
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
        Button {
            outline: true,
            size: Size::Sm,
            class: "border-0 p-0 d-inline-flex align-items-center justify-content-center",
            style: "color: {color_owned}; width: 32px; height: 32px; border-radius: 7px;",
            title: hint,
            onclick: move |_| onclick.call(()),
            "{icon}"
        }
    }
}

fn tb_icon_btn_danger(icon: &str, hint: &str, onclick: EventHandler<()>) -> Element {
    rsx! {
        Button {
            outline: true,
            size: Size::Sm,
            class: "border-0 p-0 d-inline-flex align-items-center justify-content-center",
            style: "color: var(--bs-danger); width: 32px; height: 32px; border-radius: 7px;",
            title: hint,
            onclick: move |_| onclick.call(()),
            "{icon}"
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
