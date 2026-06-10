//! 远程会话视图：RemoteView 根组件、RemoteViewport、ErrorOverlay、
//! SessionState 枚举。
//!
//! Video pipeline (pure Rust — no JavaScript WebRTC bridge):
//!   host-service → WebRTC Video Track → webrtc-rs on_track
//!   → RTP depacketize → H264 decode → JPEG → <img data-URL>
//!
//! Input/clipboard DataChannels are managed entirely in Rust.

use std::sync::Arc;

use dioxus::desktop::use_window;
use dioxus::prelude::*;
use dioxus_bootstrap_css::prelude::*;
use tokio::sync::mpsc;

use shared::frame;
use crate::components::resize::ResizeEdges;
use crate::components::titlebar::TitleBar;
use crate::config::load_config;
use crate::i18n::{t as tr, tf};
use crate::i18n::keys;
use crate::quality::{ScaleMode, QUALITY_PRESETS};
use dioxus_bootstrap_css::theme::Theme;
use crate::Page;
use crate::WindowCmd;
use crate::remote::file_transfer::{DropOverlay, FileTransferProgress, FileTransferState};
use crate::remote::viewer_loop::ConnectionStats;
use crate::screenshot;
use crate::toast::{ToastMessage, ToastOverlay};
use crate::remote::keyboard::{handle_keydown_local, handle_keyup_local, key_event_to_code};
use crate::remote::mouse::{
    frame_display_style, handle_mouse_event, image_style, MouseAction,
};
use crate::remote::toolbar::{
    handle_clipboard_toggle, handle_quality_select, RemoteToolbar,
};
use crate::remote::viewer_loop::{run_viewer, GuiCallback, InputCmd, ViewerState};

// ─── Session state ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    #[allow(dead_code)]
    Idle,
    Connecting,
    Connected,
    Error(String),
}

// ─── RemoteView ───────────────────────────────────────────────────────────

#[component]
pub fn RemoteView(
    page:        Signal<Page>,
    server_addr: Signal<String>,
    remote_id:   Signal<String>,
    quality:     Signal<usize>,
    theme:       Signal<Theme>,
    locale:      Signal<String>,
    password:    Signal<String>,
    window_cmd:  Signal<Vec<crate::WindowCmd>>,
    is_fullscreen: Signal<bool>,
) -> Element {
    let state            = use_signal(|| SessionState::Connecting);
    let frame_seq        = use_signal(|| 0u64);
    let frame_w          = use_signal(|| frame::DEFAULT_FRAME_W);
    let frame_h          = use_signal(|| frame::DEFAULT_FRAME_H);
    let mut toolbar_visible  = use_signal(|| true);
    let mut toolbar_pinned   = use_signal(|| false);
    let mut scale_mode       = use_signal(|| ScaleMode::Fit);
    let quality_idx      = use_signal(|| quality());
    let mut clipboard_synced = use_signal(|| false);
    let mut show_quality_menu = use_signal(|| false);
    let mut show_scale_menu   = use_signal(|| false);
    let right_ctrl        = use_signal(|| false);
    let local_clipboard   = use_signal(String::new);
    let mut viewer_input_tx      = use_signal(|| None::<mpsc::UnboundedSender<InputCmd>>);
    let mut viewer_quality_tx    = use_signal(|| None::<mpsc::UnboundedSender<usize>>);
    let mut viewer_disconnect_tx = use_signal(|| None::<mpsc::UnboundedSender<()>>);
    let mut viewer_clipboard_tx  = use_signal(|| None::<mpsc::UnboundedSender<String>>);
    let mut viewer_file_send_tx = use_signal(|| None::<mpsc::UnboundedSender<crate::remote::viewer_loop::FileSendCmd>>);
    let mut view_only        = use_signal(|| false);
    let show_stats_detail = use_signal(|| false);
    let mut is_dragging      = use_signal(|| false);
    let stats            = use_signal(ConnectionStats::default);
    let file_transfer    = use_signal(|| FileTransferState::Idle);
    let mut toast            = use_signal(|| None::<ToastMessage>);

    use_effect(move || {
        let srv  = server_addr();
        let rid  = remote_id();
        let pwd  = password();
        let lid  = load_config().local_id;
        let qidx = quality_idx();
        let vs = ViewerState {
            frame_seq, frame_w, frame_h,
            state, local_clipboard, clipboard_synced, page,
            view_only, window_cmd, stats,
            file_transfer, toast,
        };
        let (frame_tx,     frame_rx)     = mpsc::unbounded_channel();
        let (state_tx,     state_rx)     = mpsc::unbounded_channel();
        let (clip_tx,      clip_rx)      = mpsc::unbounded_channel();
        let (input_tx,     input_rx)     = mpsc::unbounded_channel();
        let (qt_tx,        qt_rx)        = mpsc::unbounded_channel();
        let (dc_tx,        dc_rx)        = mpsc::unbounded_channel();
        let (clip_send_tx, clip_send_rx) = mpsc::unbounded_channel();
        let (stats_tx, stats_rx)         = mpsc::unbounded_channel();
        let (file_send_tx, file_send_rx) = mpsc::unbounded_channel();
        let state_tx_clone = state_tx.clone();

        viewer_input_tx.set(Some(input_tx));
        viewer_quality_tx.set(Some(qt_tx));
        viewer_disconnect_tx.set(Some(dc_tx));
        viewer_clipboard_tx.set(Some(clip_send_tx));
        viewer_file_send_tx.set(Some(file_send_tx));

        let cb = Arc::new(GuiCallback {
            frame_tx,
            state_tx,
            clipboard_tx: clip_tx,
            stats_tx,
        });
        spawn(run_viewer(
            srv, rid, lid, qidx, pwd, vs,
            frame_rx, state_rx, clip_rx, input_rx,
            qt_rx, dc_rx, clip_send_rx,
            stats_rx, file_send_rx,
            state_tx_clone, cb,
        ));
    });

    let q_name = QUALITY_PRESETS[quality_idx().min(QUALITY_PRESETS.len().saturating_sub(1))].1;
    let cursor_style =  if view_only() { "not-allowed" } else { "default" };
    let container_overflow = use_memo(move || {
        if scale_mode() == ScaleMode::Original { "auto" } else { "hidden" }
    });
    let container_justify = use_memo(move || {
        if scale_mode() == ScaleMode::Original { "flex-start" } else { "center" }
    });

    rsx! {
        div {
            style: "display: flex; flex-direction: column; width: 100vw; height: 100vh;
                    background: var(--bs-body-bg); border: 1px solid var(--bs-border-color);
                    border-radius: 10px; overflow: hidden; user-select: none;
                    color: var(--bs-body-color);",
            if !is_fullscreen() { TitleBar { title: "RTCDesk", theme, locale } }

            div {
                tabindex: 0,
                class: "w-100 flex-fill d-flex position-relative",
                style: "min-height: 0; background: var(--bs-body-bg); outline: none;
                        cursor: {cursor_style}; overflow: {container_overflow};
                        justify-content: {container_justify}; align-items: {container_justify};",
                onkeydown: move |evt| {
                    evt.prevent_default();
                    if handle_keydown_local(&evt, right_ctrl, view_only, window_cmd) { return; }
                    // 只看模式：跳过输入转发
                    if view_only() { return; }
                    if let Some(tx) = viewer_input_tx() {
                        let _ = tx.send(InputCmd::Key {
                            key_code: key_event_to_code(&evt),
                            pressed: true,
                        });
                    }
                },
                onkeyup: move |evt| {
                    if handle_keyup_local(&evt, right_ctrl) { return; }
                    if view_only() { return; }
                    if let Some(tx) = viewer_input_tx() {
                        let _ = tx.send(InputCmd::Key {
                            key_code: key_event_to_code(&evt),
                            pressed: false,
                        });
                    }
                },
                onmousemove: move |evt| {
                    let delta = if toolbar_visible() { frame::TOOLBAR_THRESHOLD } else { 5.0 };
                    let y = evt.data().element_coordinates().y as f64;
                    toolbar_visible.set(y < delta || toolbar_pinned());
                },
                ondragover: move |evt| {
                    evt.prevent_default();
                    is_dragging.set(true);
                },
                ondragleave: move |_| {
                    is_dragging.set(false);
                },
                ondrop: move |evt| {
                    evt.prevent_default();
                    is_dragging.set(false);
                    // 桌面端 Dioxus 中 HTML5 拖放对本地文件路径支持有限
                    // 使用 rfd 文件对话框作为备选入口
                },
                oncontextmenu: move |evt| {
                    evt.prevent_default();
                },

                RemoteViewport {
                    state, frame_seq, frame_w, frame_h,
                    scale_mode, viewer_input_tx, theme, locale,
                    view_only, is_fullscreen, stats, show_stats_detail,
                }
                DropOverlay { is_dragging, locale }
                RemoteToolbar {
                    toolbar_visible, toolbar_pinned, scale_mode, quality_idx,
                    clipboard_synced, is_fullscreen, show_quality_menu, show_scale_menu,
                    view_only,
                    q_name: q_name.to_string(),
                    locale,
                    on_show_quality_menu: move |_| show_quality_menu.set(!show_quality_menu()),
                    on_show_scale_menu:   move |_| show_scale_menu.set(!show_scale_menu()),
                    on_select_quality: move |idx| {
                        handle_quality_select(idx, quality_idx, show_quality_menu, viewer_quality_tx);
                    },
                    on_select_scale: move |sm| {
                        scale_mode.set(sm);
                        show_scale_menu.set(false);
                    },
                    on_toggle_clipboard: move |_| {
                        handle_clipboard_toggle(local_clipboard, clipboard_synced, viewer_clipboard_tx);
                    },
                    on_disconnect: move |_| {
                        if let Some(tx) = viewer_disconnect_tx() { let _ = tx.send(()); }
                        window_cmd.write().push(WindowCmd::ExitFullscreen);
                        page.set(Page::Main);
                    },
                    on_toggle_fullscreen: move |_| {
                        window_cmd.write().push(WindowCmd::ToggleFullscreen);
                    },
                    on_minimize:   move |_| { window_cmd.write().push(WindowCmd::Minimize); },
                    on_toggle_pin: move |_| toolbar_pinned.set(!toolbar_pinned()),
                    on_toggle_view_only: move |_| {
                        let vo = !view_only();
                        view_only.set(vo);
                        if vo {
                            clipboard_synced.set(false);
                        }
                    },
                    on_screenshot: move |_| {
                        match screenshot::save_screenshot() {
                            Ok(path) => toast.set(Some(ToastMessage::success(
                                format!("截图已保存: {}", path)
                            ))),
                            Err(e) => toast.set(Some(ToastMessage::error(
                                format!("截图失败: {}", e)
                            ))),
                        }
                    },
                    on_open_file_dialog: move |_| {
                        let mut toast = toast;
                        let file_send_tx = viewer_file_send_tx;
                        spawn(async move {
                            let files = tokio::task::spawn_blocking(|| {
                                rfd::FileDialog::new().pick_files()
                            }).await;
                            match files {
                                Ok(Some(paths)) => {
                                    let loc = locale();
                                    let l = &loc;
                                    for path in &paths {
                                        if let Ok(meta) = std::fs::metadata(path) {
                                            if meta.len() > shared::frame::FILE_TRANSFER_MAX_SIZE {
                                                toast.set(Some(ToastMessage::error(tr(keys::REMOTE_FILE_TOO_LARGE, l).to_string())));
                                                return;
                                            }
                                        }
                                    }
                                    if let Some(tx) = file_send_tx() {
                                        let _ = tx.send(crate::remote::viewer_loop::FileSendCmd::Send { paths });
                                    }
                                }
                                _ => {}
                            }
                        });
                    },
                }
                FileTransferProgress { state: file_transfer, locale }
                ToastOverlay { toast }
                ErrorOverlay { state, page, theme, locale, window_cmd }
            }

            if !is_fullscreen() {
                ResizeEdges {}
            }
        }
    }
}

// ─── RemoteViewport ───────────────────────────────────────────────────────

#[component]
pub fn RemoteViewport(
    state:           Signal<SessionState>,
    frame_seq:       Signal<u64>,
    frame_w:         Signal<u32>,
    frame_h:         Signal<u32>,
    scale_mode:      Signal<ScaleMode>,
    viewer_input_tx: Signal<Option<mpsc::UnboundedSender<InputCmd>>>,
    theme:           Signal<Theme>,
    locale:          Signal<String>,
    view_only:       Signal<bool>,
    is_fullscreen:   Signal<bool>,
    stats:           Signal<ConnectionStats>,
    show_stats_detail: Signal<bool>,
) -> Element {
    let l: &'static str = Box::leak(locale().into_boxed_str());
    let window = use_window();
    let (w1, w2, w3) = (window.clone(), window.clone(), window.clone());
    let img_css = image_style(scale_mode(), frame_w(), frame_h());
    let display_style = frame_display_style(scale_mode(), frame_w(), frame_h());
    let _ts = theme();
    let has_frame = frame_seq() > 0;
    let frame_src = crate::frame_slot::frame_url(frame_seq());
    rsx! {
        div {
            style: "position:relative;{display_style}",
            onmousedown: move |evt| {
                if view_only() { return; }
                handle_mouse_event(
                    &evt, MouseAction::Down, &w1,
                    frame_w(), frame_h(), frame_w(), frame_h(),
                    scale_mode(), is_fullscreen(), viewer_input_tx(),
                );
            },
            onmouseup: move |evt| {
                if view_only() { return; }
                handle_mouse_event(
                    &evt, MouseAction::Up, &w2,
                    frame_w(), frame_h(), frame_w(), frame_h(),
                    scale_mode(), is_fullscreen(), viewer_input_tx(),
                );
            },
            onmousemove: move |evt| {
                if view_only() { return; }
                handle_mouse_event(
                    &evt, MouseAction::Move, &w3,
                    frame_w(), frame_h(), frame_w(), frame_h(),
                    scale_mode(), is_fullscreen(), viewer_input_tx(),
                );
            },
            onwheel: move |evt| {
                if view_only() { return; }
                if let Some(tx) = viewer_input_tx() {
                    let v = evt.data().delta().strip_units();
                    let _ = tx.send(InputCmd::Wheel { dx: v.x as i32, dy: v.y as i32 });
                }
            },

            if has_frame {
                img { src: "{frame_src}", style: "{img_css}" }
            } else {
                div {
                    style: "position: absolute; inset: 0; display: flex;
                            flex-direction: column; align-items: center; justify-content: center;
                            color: var(--bs-body-color);",
                    div {
                        style: "width: 80px; height: 80px; margin: 0 auto 20px;
                                border-radius: 50%; background: rgba(63,185,80,0.1);
                                border: 2px solid rgba(63,185,80,0.3);
                                display: flex; align-items: center; justify-content: center;
                                font-size: 32px;",
                        "⌛"
                    }
                    div {
                        style: "font-size: 18px; font-weight: 600; margin-bottom: 8px;",
                        "{tr(keys::REMOTE_CONNECTING, l)}"
                    }
                    div {
                        style: "font-size: 13px; color: var(--bs-secondary-color);",
                        "{tr(keys::REMOTE_ESTABLISHING, l)}"
                    }
                }
            }

            if state() == SessionState::Connected && has_frame {
                {
                    let s = stats();
                    let vo = view_only();
                    let l2 = l;
                    let stat_line = tf(keys::REMOTE_STATS, l2, &[
                        ("fps", &s.fps.to_string()),
                        ("rtt", &s.rtt_ms.to_string()),
                        ("mb", &format!("{:.1}", s.bytes_transferred_mb)),
                        ("w", &s.resolution_w.to_string()),
                        ("h", &s.resolution_h.to_string()),
                    ]);
                    let parts: Vec<&str> = stat_line.split('\n').collect();
                    let compact_line = parts.first().copied().unwrap_or("");
                    let detail = if parts.len() > 1 { parts[1..].join("\n") } else { String::new() };
                    rsx! {
                        div {
                            class: "position-absolute d-flex flex-column gap-1",
                            style: "bottom: 12px; right: 12px; background: rgba(15,17,22,0.85);
                                    border: 1px solid rgba(63,185,80,0.4);
                                    border-radius: 6px; padding: 4px 10px;
                                    font-size: 11px; color: #3fb950;
                                    backdrop-filter: blur(4px); cursor: pointer;",
                            onclick: move |_| show_stats_detail.set(!show_stats_detail()),
                            div {
                                class: "d-flex align-items-center gap-1",
                                div { style: "width: 6px; height: 6px; border-radius: 50%; background: #3fb950;" }
                                if vo { "{compact_line}" } else { "{tr(keys::REMOTE_CONNECTED, l2)}" }
                            }
                            if show_stats_detail() {
                                div {
                                    style: "font-size: 10px; color: #8b949e; margin-top: 2px; white-space: pre-line;",
                                    "{detail}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─── Error overlay ────────────────────────────────────────────────────────

#[component]
pub fn ErrorOverlay(
    state:      Signal<SessionState>,
    page:       Signal<Page>,
    theme:      Signal<Theme>,
    locale:     Signal<String>,
    window_cmd: Signal<Vec<WindowCmd>>,
) -> Element {
    if !matches!(state(), SessionState::Error(_)) {
        return rsx! {};
    }
    let msg = if let SessionState::Error(ref m) = state() {
        m.clone()
    } else {
        String::new()
    };
    let _ts = theme();
    let l: &'static str = Box::leak(locale().into_boxed_str());

    rsx! {
        div {
            class: "position-absolute start-50 top-50 translate-middle text-center",
            style: "background: var(--bs-tertiary-bg); border: 1px solid var(--bs-border-color);
                    border-radius: 16px; padding: 32px 40px;
                    box-shadow: 0 16px 48px rgba(0,0,0,0.6);
                    z-index: 100; min-width: 300px;",
            div { style: "font-size: 40px; margin-bottom: 14px;", "⚠" }
            div {
                style: "font-size: 16px; font-weight: 600; margin-bottom: 8px; color: var(--bs-body-color);",
                "{tr(keys::REMOTE_ERR_TITLE, l)}"
            }
            Alert {
                color: Color::Danger,
                class: "mb-3",
                "{msg}"
            }
            Button {
                color: Color::Primary,
                onclick: move |_| {
                    window_cmd.write().push(WindowCmd::ExitFullscreen);
                    page.set(Page::Main);
                },
                "{tr(keys::REMOTE_ERR_BACK, l)}"
            }
        }
    }
}
