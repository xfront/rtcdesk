//! RTCDesk Desktop GUI Client
//!
//! Dioxus-based remote desktop viewer, replicating the Compose Multiplatform UI.
//! Connects to a signaling server as a viewer and renders the remote desktop stream
//! with a Remmina-style floating toolbar, quality/scale controls, and clipboard sync.

mod frame;
mod signal;
mod viewer;

use std::cmp;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use dioxus::desktop::tao::window::Fullscreen;
use dioxus::desktop::{self, use_window};
use dioxus::prelude::*;
use dioxus_html::input_data::MouseButton;
use keyboard_types::{Code, Key};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};

use tokio::sync::mpsc;
use viewer::ViewerCallback;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const HEADER_SVG: Asset = asset!("/assets/header.svg");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

// ─── Constants ────────────────────────────────────────────────────────

const ACCENT_GREEN: &str = "#0F9D58";
const DANGER_RED: &str = "#E53935";
const TEXT_WHITE: &str = "#E0E0E0";
const PANEL_BG: &str = "#242442";
const TOOLBAR_BG_TRANSPARENT: &str = "rgba(26, 26, 46, 0.85)";

#[rustfmt::skip]
const QUALITY_PRESETS: &[(u32, &str, u32, u32, u32, u32)] = &[
    (70, "最佳画质", 1920, 1080, 30, 10_000),
    (50, "均衡",     1280, 720,  25, 5_000),
    (30, "流畅优先", 1024, 576,  20, 2_000),
    (10, "低带宽",    800, 450,  15, 500),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum ScaleMode {
    Fit,
    Stretch,
    Original,
}

impl ScaleMode {
    fn label(self) -> &'static str {
        match self {
            ScaleMode::Fit => "自适应",
            ScaleMode::Stretch => "拉伸填充",
            ScaleMode::Original => "原始大小",
        }
    }
}

const SCALE_MODES: &[ScaleMode] = &[ScaleMode::Fit, ScaleMode::Stretch, ScaleMode::Original];

type QualityIdx = usize;

#[derive(Debug, Clone, PartialEq)]
enum Page {
    Connect,
    Remote,
}

#[derive(Debug, Clone, PartialEq)]
enum SessionState {
    Idle,
    Connecting,
    Connected,
    Error(String),
}

// ─── Viewer callback bridge ───────────────────────────────────────────

struct GuiCallback {
    frame_tx: mpsc::UnboundedSender<(Vec<u8>, u32, u32, u32, u32)>,
    state_tx: mpsc::UnboundedSender<SessionState>,
    clipboard_tx: mpsc::UnboundedSender<String>,
}

impl ViewerCallback for GuiCallback {
    fn on_connected(&self) {
        if let Err(e) = self.state_tx.send(SessionState::Connected) {
            warn!("[GUI] state_tx send error: {}", e);
        }
    }
    fn on_disconnected(&self) {
        if let Err(e) = self.state_tx.send(SessionState::Error("连接已断开".into())) {
            warn!("[GUI] state_tx send error: {}", e);
        }
    }
    fn on_frame(&self, data: Vec<u8>, orig_w: u32, orig_h: u32, frame_w: u32, frame_h: u32) {
        if let Err(e) = self.frame_tx.send((data, orig_w, orig_h, frame_w, frame_h)) {
            warn!("[GUI] frame_tx send error: {}", e);
        }
    }
    fn on_connection_state(&self, state: String) {
        info!("[GUI] state: {}", state);
    }
    fn on_error(&self, message: String) {
        error!("[GUI] {}", message);
        if let Err(e) = self.state_tx.send(SessionState::Error(message)) {
            warn!("[GUI] state_tx error send failed: {}", e);
        }
    }
    fn on_clipboard_text(&self, text: String) {
        if let Err(e) = self.clipboard_tx.send(text) {
            warn!("[GUI] clipboard_tx send error: {}", e);
        }
    }
    fn on_clipboard_file_uris(&self, uris: String) {
        info!("[GUI] files: {}", uris);
    }
}

enum InputCmd {
    Mouse {
        x: f64,
        y: f64,
        button: u8,
        pressed: bool,
    },
    Key {
        key_code: u32,
        pressed: bool,
    },
    Wheel {
        dx: i32,
        dy: i32,
    },
}

// ─── Entry point ──────────────────────────────────────────────────────

fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            desktop::Config::new().with_window(
                desktop::WindowBuilder::new()
                    .with_title("RTCDesk")
                    .with_decorations(false) // start undecorated; we add them back on startup
                    .with_min_inner_size(desktop::LogicalSize::new(frame::MIN_WINDOW_W as i32, frame::MIN_WINDOW_H as i32))
                    .with_inner_size(desktop::LogicalSize::new(frame::DEFAULT_WINDOW_W as i32, frame::DEFAULT_WINDOW_H as i32)),
            ),
        )
        .launch(app);
}

// ─── Root component ───────────────────────────────────────────────────

#[component]
fn app() -> Element {
    let mut css_injected = use_signal(|| false);
    // Ensure body fills the viewport without any margin/padding offset,
    // otherwise client_coordinates won't align with the remote-view container.
    use_effect(move || {
        if css_injected() {
            return;
        }
        css_injected.set(true);
        let _ = document::eval(
            r#"
            var s = document.createElement('style');
            s.textContent = 'html,body{margin:0;padding:0;width:100%;height:100%;overflow:hidden;background:#000;}';
            document.head.appendChild(s);
            "#,
        );
        // document::eval returns Result but in Dioxus it's fire-and-forget in wasm context.
        // We can't meaningfully handle the error here, so it's left as `let _`.
    });

    let cfg = load_config();
    let page = use_signal(|| Page::Connect);
    let server_addr = use_signal(move || cfg.server_addr.clone());
    let remote_id = use_signal(move || cfg.remote_id.clone());
    let quality = use_signal(|| frame::DEFAULT_QUALITY_IDX);

    match page() {
        Page::Connect => rsx! { ConnectionForm { page, server_addr, remote_id, quality } },
        Page::Remote => rsx! { RemoteView { page, server_addr, remote_id, quality } },
    }
}

// ─── Connection form ──────────────────────────────────────────────────

#[component]
fn ConnectionForm(
    page: Signal<Page>,
    server_addr: Signal<String>,
    remote_id: Signal<String>,
    quality: Signal<QualityIdx>,
) -> Element {
    let mut connecting = use_signal(|| false);
    let mut error_msg = use_signal(String::new);
    let quality_idx = quality().min(QUALITY_PRESETS.len().saturating_sub(1));
    let q = &QUALITY_PRESETS[quality_idx];
    let q_label = format!("{} ({}x{} {}FPS)", q.1, q.2, q.3, q.4);
    let blocked = server_addr().is_empty() || remote_id().is_empty() || connecting();
    let btn_bg = if blocked { "#444" } else { ACCENT_GREEN };
    let btn_cursor = if blocked { "not-allowed" } else { "pointer" };
    let btn_opacity = if blocked { "0.6" } else { "1" };
    let window = use_window();
    apply_fullscreen(&window, false);

    rsx! {
        div { style: "display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;background:linear-gradient(135deg,#1a1a2e 0%,#16213e 100%);font-family:system-ui,sans-serif;padding:20px;",
            div { style: "text-align:center;margin-bottom:40px;",
                div { style: "font-size:48px;margin-bottom:8px;", "🖥️" }
                h1 { style: "color:{TEXT_WHITE};font-size:28px;font-weight:600;margin:0;", "RTCDesk" }
                p { style: "color:#888;font-size:14px;margin:8px 0 0;", "远程桌面控制" }
            }
            div { style: "background:{PANEL_BG};border-radius:16px;padding:32px;width:100%;max-width:420px;box-shadow:0 8px 32px rgba(0,0,0,0.3);",
                label { style: "color:{TEXT_WHITE};font-size:13px;margin-bottom:6px;display:block;", "服务器地址" }
                input { r#type: "text",
                    style: "width:100%;padding:12px 14px;border-radius:10px;border:1px solid #444;background:rgba(255,255,255,0.08);color:{TEXT_WHITE};font-size:14px;outline:none;box-sizing:border-box;margin-bottom:16px;",
                    placeholder: "例如: 192.168.1.100:9009",
                    value: "{server_addr}",
                    oninput: move |evt| server_addr.set(evt.value()),
                }
                label { style: "color:{TEXT_WHITE};font-size:13px;margin-bottom:6px;display:block;", "远程设备 ID" }
                input { r#type: "text",
                    style: "width:100%;padding:12px 14px;border-radius:10px;border:1px solid #444;background:rgba(255,255,255,0.08);color:{TEXT_WHITE};font-size:14px;outline:none;box-sizing:border-box;margin-bottom:20px;",
                    placeholder: "输入对方设备 ID",
                    value: "{remote_id}",
                    oninput: move |evt| remote_id.set(evt.value()),
                }
                label { style: "color:{TEXT_WHITE};font-size:13px;margin-bottom:6px;display:block;", "画质设置" }
                div { style: "display:flex;gap:6px;margin-bottom:8px;flex-wrap:wrap;",
                    for (idx, preset) in QUALITY_PRESETS.iter().enumerate() {
                        {
                            let active = idx == quality_idx;
                            let border = if active { "#0F9D58" } else { "#444" };
                            let bg = if active { "rgba(15,157,88,0.15)" } else { "rgba(255,255,255,0.05)" };
                            let c = if active { "#0F9D58" } else { "#E0E0E0" };
                            rsx! {
                                button { key: "{idx}",
                                    style: "flex:1;padding:8px 4px;border-radius:8px;border:1px solid {border};background:{bg};color:{c};font-size:11px;cursor:pointer;white-space:nowrap;min-width:0;",
                                    onclick: move |_| quality.set(idx),
                                    "{preset.1}"
                                }
                            }
                        }
                    }
                }
                div { style: "color:#888;font-size:12px;margin-bottom:20px;text-align:center;", "{q_label}" }
                button {
                    style: "width:100%;padding:14px;border-radius:12px;border:none;background:{btn_bg};color:white;font-size:16px;font-weight:600;cursor:{btn_cursor};transition:all 0.3s;opacity:{btn_opacity};",
                    disabled: blocked,
                    onclick: move |_| {
                        if blocked { return; }
                        save_config(&AppConfig { server_addr: server_addr(), remote_id: remote_id() });
                        connecting.set(true); error_msg.set(String::new()); page.set(Page::Remote);
                    },
                    if connecting() { "连接中..." } else { "连接" }
                }
                if !error_msg().is_empty() {
                    div { style: "color:{DANGER_RED};font-size:13px;text-align:center;margin-top:16px;padding:10px;background:rgba(229,57,53,0.1);border-radius:8px;", "{error_msg}" }
                }
            }
        }
    }
}

// ─── Fullscreen helper ────────────────────────────────────────────────

fn apply_fullscreen(window: &dioxus::desktop::DesktopService, fs: bool) {
    let win: &dioxus::desktop::tao::window::Window = &**window;
    if fs {
        win.set_fullscreen(Some(Fullscreen::Borderless(None)));
        win.set_decorations(false);
    } else {
        win.set_fullscreen(None);
        win.set_decorations(true);
    }
}

// ─── Toolbar callback helpers ─────────────────────────────────────────

fn handle_quality_select(
    idx: QualityIdx,
    mut quality_idx: Signal<QualityIdx>,
    mut show_quality_menu: Signal<bool>,
    viewer_quality_tx: Signal<Option<mpsc::UnboundedSender<QualityIdx>>>,
) {
    quality_idx.set(idx);
    show_quality_menu.set(false);
    if let Some(tx) = viewer_quality_tx() {
        if let Err(e) = tx.send(idx) {
            warn!("[GUI] quality_tx send error: {}", e);
        }
    }
}

fn handle_clipboard_toggle(
    mut local_clipboard: Signal<String>,
    mut clipboard_synced: Signal<bool>,
    viewer_clipboard_tx: Signal<Option<mpsc::UnboundedSender<String>>>,
) {
    if let Ok(text) = get_system_clipboard() {
        if !text.is_empty() && text != local_clipboard() {
            local_clipboard.set(text.clone());
            if let Some(tx) = viewer_clipboard_tx() {
                if let Err(e) = tx.send(text) {
                    warn!("[GUI] clipboard_tx send error: {}", e);
                }
            }
            clipboard_synced.set(true);
        }
    }
    clipboard_synced.set(!clipboard_synced());
}

// ─── ViewerState holds signals that the async event loop mutates ─────

struct ViewerState {
    frame_b64: Signal<String>,
    orig_w: Signal<u32>,
    orig_h: Signal<u32>,
    frame_w: Signal<u32>,
    frame_h: Signal<u32>,
    state: Signal<SessionState>,
    local_clipboard: Signal<String>,
    clipboard_synced: Signal<bool>,
    page: Signal<Page>,
}

// ─── Async viewer event loop ──────────────────────────────────────────

/// All channels created by the caller and passed in; ensures signals are
/// populated synchronously before the async task starts.
async fn run_viewer(
    srv: String,
    rid: String,
    qidx: usize,
    mut vs: ViewerState,
    mut frame_rx: mpsc::UnboundedReceiver<(Vec<u8>, u32, u32, u32, u32)>,
    mut state_rx: mpsc::UnboundedReceiver<SessionState>,
    mut clip_rx: mpsc::UnboundedReceiver<String>,
    mut input_rx: mpsc::UnboundedReceiver<InputCmd>,
    mut qt_rx: mpsc::UnboundedReceiver<QualityIdx>,
    mut dc_rx: mpsc::UnboundedReceiver<()>,
    mut clip_send_rx: mpsc::UnboundedReceiver<String>,
    state_tx_clone: mpsc::UnboundedSender<SessionState>,
    cb: Arc<GuiCallback>,
) {
    let qidx_safe = qidx.min(QUALITY_PRESETS.len().saturating_sub(1));
    let mut viewer = viewer::RtcViewer::new(format!("gui-{}", rand_id()), rid);
    viewer.set_callback(cb);
    viewer.set_pic_quality(QUALITY_PRESETS[qidx_safe].0);
    viewer.set_resolution(QUALITY_PRESETS[qidx_safe].2, QUALITY_PRESETS[qidx_safe].3);
    viewer.set_fps(QUALITY_PRESETS[qidx_safe].4);
    if let Err(e) = viewer.connect(srv, load_stun_servers()).await {
        if let Err(se) = state_tx_clone.send(SessionState::Error(format!("连接失败: {}", e))) {
            warn!("[Gui] state_tx send error on connect failure: {}", se);
        }
        return;
    }
    // Clipboard auto-sync: track last sent hash to prevent echo loops.
    let mut last_clip_sent_hash: u64 = 0;
    let mut clip_poll = tokio::time::interval(Duration::from_millis(frame::CLIPBOARD_WRITE_POLL_MS));
    // Skip the first tick (fires immediately).
    clip_poll.tick().await;
    loop {
        tokio::select! {
            Some((data, ow, oh, w, h)) = frame_rx.recv() => {
                vs.frame_b64.set(format!("data:image/jpeg;base64,{}", BASE64.encode(&data)));
                vs.orig_w.set(ow);
                vs.orig_h.set(oh);
                vs.frame_w.set(w);
                vs.frame_h.set(h);
            }
            Some(s) = state_rx.recv() => { vs.state.set(s); }
            Some(text) = clip_rx.recv() => {
                vs.local_clipboard.set(text.clone());
                vs.clipboard_synced.set(true);
                last_clip_sent_hash = text_hash(&text);
                if let Err(e) = set_system_clipboard(&text) {
                    warn!("[Gui] set_system_clipboard error: {}", e);
                }
            }
            Some(text) = clip_send_rx.recv() => {
                if let Err(e) = viewer.send_clipboard_data(build_clipboard_msg(&text)).await {
                    error!("[Gui] send_clipboard_data error: {}", e);
                }
                last_clip_sent_hash = text_hash(&text);
            }
            Some(cmd) = input_rx.recv() => match cmd {
                InputCmd::Mouse { x, y, button, pressed } => {
                    if let Err(e) = viewer.send_mouse_event(x, y, button, pressed).await {
                        error!("[Gui] send_mouse_event error: {}", e);
                    }
                }
                InputCmd::Key { key_code, pressed } => {
                    if let Err(e) = viewer.send_key_event(key_code, pressed).await {
                        error!("[Gui] send_key_event error: {}", e);
                    }
                }
                InputCmd::Wheel { dx, dy } => {
                    if let Err(e) = viewer.send_wheel_event(dx, dy).await {
                        error!("[Gui] send_wheel_event error: {}", e);
                    }
                }
            },
            Some(q) = qt_rx.recv() => {
                let qsafe = q.min(QUALITY_PRESETS.len().saturating_sub(1));
                let qref = &QUALITY_PRESETS[qsafe];
                viewer.set_pic_quality(qref.0);
                viewer.set_resolution(qref.2, qref.3);
                viewer.set_fps(qref.4);
                let max_dim = cmp::max(qref.2, qref.3);
                if let Err(e) = viewer.send_quality_update(qref.0, max_dim, qref.4).await {
                    error!("[Gui] send_quality_update error: {}", e);
                }
            }
            Some(()) = dc_rx.recv() => {
                viewer.disconnect().await;
                vs.page.set(Page::Connect);
                return;
            }
            _ = clip_poll.tick() => {
                if let Ok(text) = get_system_clipboard() {
                    let h = text_hash(&text);
                    if !text.is_empty() && h != last_clip_sent_hash {
                        last_clip_sent_hash = h;
                        if let Err(e) = viewer.send_clipboard_data(build_clipboard_msg(&text)).await {
                            warn!("[Gui] auto clip sync error: {}", e);
                        }
                    }
                }
            }
            else => break,
        }
    }
}

// ─── Viewport sub-component ───────────────────────────────────────────

#[component]
fn RemoteViewport(
    state: Signal<SessionState>,
    frame_b64: Signal<String>,
    orig_w: Signal<u32>,
    orig_h: Signal<u32>,
    frame_w: Signal<u32>,
    frame_h: Signal<u32>,
    scale_mode: Signal<ScaleMode>,
    has_frame: bool,
    viewer_input_tx: Signal<Option<mpsc::UnboundedSender<InputCmd>>>,
) -> Element {
    let window = use_window();
    let (w1, w2, w3, w4) = (window.clone(), window.clone(), window.clone(), window.clone());
    let img_css = image_style(scale_mode());
    let display_style = frame_display_style(scale_mode());
    let s = state();

    rsx! {
        div { style: "position:relative;display:flex;align-items:center;justify-content:center;{display_style}",
            onmousedown: move |evt| {
                handle_mouse_event(&evt, MouseAction::Down, &w1, orig_w(), orig_h(), frame_w(), frame_h(), scale_mode(), viewer_input_tx());
            },
            onmouseup: move |evt| {
                handle_mouse_event(&evt, MouseAction::Up, &w2, orig_w(), orig_h(), frame_w(), frame_h(), scale_mode(), viewer_input_tx());
            },
            onmousemove: move |evt| {
                handle_mouse_event(&evt, MouseAction::Move, &w3, orig_w(), orig_h(), frame_w(), frame_h(), scale_mode(), viewer_input_tx());
            },
            onwheel: move |evt| {
                if let Some(tx) = viewer_input_tx() {
                    let v = evt.data().delta().strip_units();
                    if let Err(e) = tx.send(InputCmd::Wheel { dx: v.x as i32, dy: v.y as i32 }) {
                        warn!("[GUI] wheel tx send error: {}", e);
                    }
                }
            },
            if !has_frame {
                div { style: "text-align:center;padding:40px;color:{TEXT_WHITE};",
                    div { style: "font-size:64px;margin-bottom:16px;", "🔗" }
                    div { style: "font-size:18px;margin-bottom:8px;", "正在连接..." }
                    div { style: "font-size:13px;color:#888;", "正在建立安全连接，请稍候" }
                }
            } else {
                img { src: "{frame_b64}", style: "{img_css}" }
            }
            if s == SessionState::Connected && has_frame {
                div { style: "position:absolute;bottom:12px;right:12px;background:rgba(0,0,0,0.6);border-radius:6px;padding:4px 10px;font-size:11px;color:{ACCENT_GREEN};",
                    "● 已连接"
                }
            }
        }
    }
}

// ─── Error overlay component ──────────────────────────────────────────

#[component]
fn ErrorOverlay(state: Signal<SessionState>, page: Signal<Page>) -> Element {
    if !matches!(state(), SessionState::Error(_)) {
        return rsx! {};
    }
    let msg = if let SessionState::Error(ref m) = state() {
        m.clone()
    } else {
        String::new()
    };
    rsx! {
        div { style: "position:absolute;top:50%;left:50%;transform:translate(-50%,-50%);background:{PANEL_BG};border-radius:16px;padding:32px;text-align:center;box-shadow:0 8px 32px rgba(0,0,0,0.5);z-index:100;",
            div { style: "font-size:48px;margin-bottom:16px;", "⚠️" }
            div { style: "color:{TEXT_WHITE};font-size:16px;margin-bottom:8px;", "连接错误" }
            div { style: "color:{DANGER_RED};font-size:13px;margin-bottom:20px;", "{msg}" }
            button { style: "padding:10px 32px;border-radius:10px;border:none;background:{ACCENT_GREEN};color:white;font-size:14px;cursor:pointer;",
                onclick: move |_| page.set(Page::Connect), "返回"
            }
        }
    }
}

// ─── Remote desktop view ──────────────────────────────────────────────

#[component]
fn RemoteView(
    page: Signal<Page>,
    server_addr: Signal<String>,
    remote_id: Signal<String>,
    quality: Signal<QualityIdx>,
) -> Element {
    let mut state = use_signal(|| SessionState::Connecting);
    let mut frame_b64 = use_signal(String::new);
    let mut orig_w = use_signal(|| frame::DEFAULT_FRAME_W);
    let mut orig_h = use_signal(|| frame::DEFAULT_FRAME_H);
    let mut frame_w = use_signal(|| frame::DEFAULT_FRAME_W);
    let mut frame_h = use_signal(|| frame::DEFAULT_FRAME_H);
    let mut toolbar_visible = use_signal(|| true);
    let mut toolbar_pinned = use_signal(|| false);
    let mut scale_mode = use_signal(|| ScaleMode::Fit);
    let mut is_fullscreen = use_signal(|| false);
    let mut quality_idx = use_signal(|| quality());
    let mut clipboard_synced = use_signal(|| false);
    let mut show_quality_menu = use_signal(|| false);
    let mut show_scale_menu = use_signal(|| false);
    let mut right_ctrl = use_signal(|| false);
    let mut local_clipboard = use_signal(String::new);
    let mut viewer_input_tx = use_signal(|| None::<mpsc::UnboundedSender<InputCmd>>);
    let mut viewer_quality_tx = use_signal(|| None::<mpsc::UnboundedSender<QualityIdx>>);
    let mut viewer_disconnect_tx = use_signal(|| None::<mpsc::UnboundedSender<()>>);
    let mut viewer_clipboard_tx = use_signal(|| None::<mpsc::UnboundedSender<String>>);

    use_effect(move || {
        let srv = server_addr();
        let rid = remote_id();
        let qidx = quality_idx();
        let vs = ViewerState {
            frame_b64,
            orig_w,
            orig_h,
            frame_w,
            frame_h,
            state,
            local_clipboard,
            clipboard_synced,
            page,
        };

        // Create channels synchronously so signals are populated before render completes
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<(Vec<u8>, u32, u32, u32, u32)>();
        let (state_tx, state_rx) = mpsc::unbounded_channel::<SessionState>();
        let (clip_tx, clip_rx) = mpsc::unbounded_channel::<String>();
        let (input_tx, input_rx) = mpsc::unbounded_channel::<InputCmd>();
        let (qt_tx, qt_rx) = mpsc::unbounded_channel::<QualityIdx>();
        let (dc_tx, dc_rx) = mpsc::unbounded_channel::<()>();
        let (clip_send_tx, clip_send_rx) = mpsc::unbounded_channel::<String>();
        let state_tx_clone = state_tx.clone();

        viewer_input_tx.set(Some(input_tx));
        viewer_quality_tx.set(Some(qt_tx));
        viewer_disconnect_tx.set(Some(dc_tx));
        viewer_clipboard_tx.set(Some(clip_send_tx));

        let cb = Arc::new(GuiCallback {
            frame_tx,
            state_tx,
            clipboard_tx: clip_tx,
        });
        spawn(run_viewer(
            srv,
            rid,
            qidx,
            vs,
            frame_rx,
            state_rx,
            clip_rx,
            input_rx,
            qt_rx,
            dc_rx,
            clip_send_rx,
            state_tx_clone,
            cb,
        ));
    });

    let window = use_window();
    let (w4, w5, w_fs) = (window.clone(), window.clone(), window.clone());
    use_effect(move || apply_fullscreen(&w_fs, is_fullscreen()));
    let q_name = QUALITY_PRESETS[quality_idx().min(QUALITY_PRESETS.len().saturating_sub(1))].1;
    let has_frame = !frame_b64().is_empty();

    rsx! {
        div { tabindex: 0,
            style: "width:100vw;height:100vh;background:#000;display:flex;align-items:center;justify-content:center;overflow:hidden;position:relative;cursor:default;font-family:system-ui;outline:none;",
            onmousemove: move |evt| {
                let delta = if toolbar_visible() { frame::TOOLBAR_THRESHOLD } else { 5.0 };
                let y = evt.data().client_coordinates().y as f64;
                toolbar_visible.set(y < delta || toolbar_pinned());
            },
            onkeydown: move |evt| {
                evt.prevent_default();
                if handle_keydown_local(&evt, is_fullscreen, right_ctrl, &w5) { return; }
                if let Some(tx) = viewer_input_tx() {
                    if let Err(e) = tx.send(InputCmd::Key {
                        key_code: key_event_to_code(&evt), pressed: true }) {
                        warn!("[GUI] keydown tx error: {}", e);
                    }
                }
            },
            onkeyup: move |evt| {
                if handle_keyup_local(&evt, right_ctrl) { return; }
                if let Some(tx) = viewer_input_tx() {
                    if let Err(e) = tx.send(InputCmd::Key {
                        key_code: key_event_to_code(&evt), pressed: false }) {
                        warn!("[GUI] keyup tx error: {}", e);
                    }
                }
            },
            RemoteViewport {
                state, frame_b64, orig_w, orig_h, frame_w, frame_h, scale_mode, has_frame,
                viewer_input_tx,
            }
            RemoteToolbar {
                toolbar_visible, toolbar_pinned, scale_mode, quality_idx,
                clipboard_synced, is_fullscreen, show_quality_menu, show_scale_menu,
                q_name: q_name.to_string(),
                on_show_quality_menu: move |_| show_quality_menu.set(!show_quality_menu()),
                on_show_scale_menu: move |_| show_scale_menu.set(!show_scale_menu()),
                on_select_quality: move |idx| handle_quality_select(idx, quality_idx, show_quality_menu, viewer_quality_tx),
                on_select_scale: move |sm| { scale_mode.set(sm); show_scale_menu.set(false); },
                on_toggle_clipboard: move |_| handle_clipboard_toggle(local_clipboard, clipboard_synced, viewer_clipboard_tx),
                on_disconnect: move |_| {
                    if let Some(tx) = viewer_disconnect_tx() {
                        if let Err(e) = tx.send(()) {
                            warn!("[GUI] dc_tx send error: {}", e);
                        }
                    }
                    page.set(Page::Connect);
                },
                on_toggle_fullscreen: move |_| {
                    let fs = !is_fullscreen(); is_fullscreen.set(fs);
                    apply_fullscreen(&w4, fs);
                },
                on_minimize: move |_| {
                    window.set_minimized(true)
                },
                on_toggle_pin: move |_| toolbar_pinned.set(!toolbar_pinned()),
            }
            ErrorOverlay { state, page }
        }
    }
}

// ─── Toolbar component ────────────────────────────────────────────────

#[component]
fn RemoteToolbar(
    toolbar_visible: Signal<bool>,
    toolbar_pinned: Signal<bool>,
    scale_mode: Signal<ScaleMode>,
    quality_idx: Signal<QualityIdx>,
    clipboard_synced: Signal<bool>,
    is_fullscreen: Signal<bool>,
    show_quality_menu: Signal<bool>,
    show_scale_menu: Signal<bool>,
    q_name: String,
    on_show_quality_menu: EventHandler<()>,
    on_show_scale_menu: EventHandler<()>,
    on_select_quality: EventHandler<QualityIdx>,
    on_select_scale: EventHandler<ScaleMode>,
    on_toggle_clipboard: EventHandler<()>,
    on_disconnect: EventHandler<()>,
    on_toggle_fullscreen: EventHandler<()>,
    on_minimize: EventHandler<()>,
    on_toggle_pin: EventHandler<()>,
) -> Element {
    let ty = if toolbar_visible() || toolbar_pinned() {
        "0px"
    } else {
        "-52px"
    };
    rsx! {
        div {
            style: "position:absolute;top:0;left:50%;transform:translateX(-50%) translateY({ty});background:{TOOLBAR_BG_TRANSPARENT};border-radius:0 0 12px 12px;padding:6px 12px;display:flex;align-items:center;gap:4px;z-index:50;transition:transform 0.25s ease;box-shadow:0 4px 16px rgba(0,0,0,0.3);backdrop-filter:blur(8px);",
            onmouseenter: move |_| { if !toolbar_pinned() { toolbar_visible.set(true); } },
            onmouseleave: move |_| { if !toolbar_pinned() { toolbar_visible.set(false); } },

            // Quality menu
            div { style: "position:relative;",
                {tb_btn("🖥", &format!("画质: {}", q_name), show_quality_menu(), on_show_quality_menu)}
                if show_quality_menu() {
                    div {
                        style: "position:absolute;top:100%;left:50%;transform:translateX(-50%);margin-top:4px;background:{PANEL_BG};border-radius:10px;padding:4px;min-width:140px;box-shadow:0 4px 12px rgba(0,0,0,0.4);z-index:100;",
                        onmouseleave: move |_| show_quality_menu.set(false),
                        for (idx, preset) in QUALITY_PRESETS.iter().enumerate() {
                            {
                                let a = idx == quality_idx();
                                let c = if a { ACCENT_GREEN } else { TEXT_WHITE };
                                let b = if a { "rgba(15,157,88,0.1)" } else { "transparent" };
                                rsx! {
                                    div { key: "q_{idx}",
                                        style: "padding:8px 12px;border-radius:8px;cursor:pointer;color:{c};font-size:13px;white-space:nowrap;background:{b};",
                                        onmousedown: move |_| on_select_quality.call(idx),
                                        "{preset.1}  {preset.2}x{preset.3}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Scale menu
            div { style: "position:relative;",
                {tb_btn("🔍", &format!("缩放: {}", scale_mode().label()), show_scale_menu(), on_show_scale_menu)}
                if show_scale_menu() {
                    div {
                        style: "position:absolute;top:100%;left:50%;transform:translateX(-50%);margin-top:4px;background:{PANEL_BG};border-radius:10px;padding:4px;min-width:120px;box-shadow:0 4px 12px rgba(0,0,0,0.4);z-index:100;",
                        onmouseleave: move |_| show_scale_menu.set(false),
                        for sm in SCALE_MODES {
                            {
                                let a = *sm == scale_mode();
                                let c = if a { ACCENT_GREEN } else { TEXT_WHITE };
                                let b = if a { "rgba(15,157,88,0.1)" } else { "transparent" };
                                let s = *sm;
                                rsx! {
                                    div { key: "sm_{s:?}",
                                        style: "padding:8px 12px;border-radius:8px;cursor:pointer;color:{c};font-size:13px;white-space:nowrap;background:{b};",
                                        onmousedown: move |_| on_select_scale.call(s),
                                        "{s.label()}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { style: "width:1px;height:22px;background:rgba(255,255,255,0.15);margin:0 4px;" }
            {tb_btn("📋", if clipboard_synced() { "剪贴板已同步" } else { "同步剪贴板" }, clipboard_synced(), on_toggle_clipboard)}
            div { style: "width:1px;height:22px;background:rgba(255,255,255,0.15);margin:0 4px;" }
            {tb_btn_colored("✕", "断开连接", DANGER_RED, on_disconnect)}
            {tb_btn("⛶", if is_fullscreen() { "退出全屏" } else { "全屏" }, is_fullscreen(), on_toggle_fullscreen)}
            {tb_btn("━", "最小化", false, on_minimize)}
            {tb_btn(if toolbar_pinned() { "📍" } else { "📌" }, if toolbar_pinned() { "已锁定" } else { "锁定工具栏" }, toolbar_pinned(), on_toggle_pin)}
        }
    }
}

fn tb_btn(icon: &str, hint: &str, active: bool, onclick: EventHandler<()>) -> Element {
    let bg = if active {
        "rgba(255,255,255,0.15)"
    } else {
        "transparent"
    };
    rsx! {
        div { style: "width:34px;height:34px;border-radius:8px;display:flex;align-items:center;justify-content:center;cursor:pointer;font-size:16px;user-select:none;background:{bg};color:{TEXT_WHITE};",
            title: hint, onclick: move |_| onclick.call(()), "{icon}"
        }
    }
}

fn tb_btn_colored(icon: &str, hint: &str, color: &str, onclick: EventHandler<()>) -> Element {
    rsx! {
        div { style: "width:34px;height:34px;border-radius:8px;display:flex;align-items:center;justify-content:center;cursor:pointer;font-size:16px;user-select:none;color:{color};",
            title: hint, onclick: move |_| onclick.call(()), "{icon}"
        }
    }
}

// ─── Mouse helpers ────────────────────────────────────────────────────

#[derive(PartialEq, Copy, Clone, Debug)]
enum MouseAction { Down, Up, Move }

fn handle_mouse_event(
    evt: &Event<MouseData>,
    action: MouseAction,
    win: &dioxus::desktop::DesktopService,
    orig_w: u32,
    orig_h: u32,
    frame_w: u32,
    frame_h: u32,
    scale_mode: ScaleMode,
    tx: Option<mpsc::UnboundedSender<InputCmd>>,
) {
    let (ew, eh) = logical_size(win);
    let (rx, ry) = relative_coords(evt, orig_w, orig_h, frame_w, frame_h, scale_mode, ew, eh);
    let cc = evt.data().client_coordinates();
    let ec = evt.data().element_coordinates();

    let btn = if action == MouseAction::Move { frame::BTN_MOVE } else { mouse_button(evt) };
    match tx {
        Some(tx) => {
            debug!("MOUSE {:?} ctl=({:.0},{:.0}) elm=({:.0},{:.0}) d=({:.0},{:.0}) win=({:.0},{:.0}) orig={}x{} frm={}x{} -> rem=({:.0},{:.0})",
                action, cc.x, cc.y, ec.x, ec.y, cc.x-ec.x, cc.y-ec.y, ew, eh, orig_w, orig_h, frame_w, frame_h, rx, ry);
            if let Err(e) = tx.send(InputCmd::Mouse { x: rx, y: ry, button: btn, pressed: action == MouseAction::Down }) {
                warn!("[GUI] mouse tx send error: {}", e);
            }
        }
        None => warn!("MOUSE {:?} dropped: input channel not ready", action),
    }
}

fn mouse_button(evt: &Event<MouseData>) -> u8 {
    evt.data()
        .trigger_button()
        .map(|b| match b {
            MouseButton::Primary => frame::BTN_LEFT,
            MouseButton::Auxiliary => frame::BTN_MIDDLE,
            MouseButton::Secondary => frame::BTN_RIGHT,
            _ => frame::BTN_LEFT,
        })
        .unwrap_or(frame::BTN_LEFT)
}

/// Map element coordinates (relative to the viewport div) to the remote host's
/// **original** screen coordinates, accounting for both the current ScaleMode
/// (how the frame is rendered in the window) and any down-scaling that the host
/// applied when capturing (frame_w vs orig_w).
///
/// ── Pipeline ──────────────────────────────────────────────────────────
///   element pixel  ──[ScaleMode]──▶  frame pixel  ──[scale factor]──▶  remote screen pixel
///
/// For example, a 1920×1080 remote screen captured at "均衡" quality sends
/// 1280×720 frames.  A click at the window center maps to (640,360) in frame
/// space, then (960,540) in the actual 1920×1080 remote screen space.
fn relative_coords(
    evt: &Event<MouseData>,
    orig_w: u32,  // remote monitor actual width
    orig_h: u32,  // remote monitor actual height
    frame_w: u32, // JPEG frame width (may be < orig_w)
    frame_h: u32, // JPEG frame height (may be < orig_h)
    scale: ScaleMode,
    elem_w: f64,
    elem_h: f64,
) -> (f64, f64) {
    let ec = evt.data().element_coordinates();
    let cx = ec.x as f64;
    let cy = ec.y as f64;

    let fw = frame_w as f64;
    let fh = frame_h as f64;
    let ow = orig_w as f64;
    let oh = orig_h as f64;

    if elem_w <= 1.0 || elem_h <= 1.0 || fw <= 1.0 || fh <= 1.0 {
        return (cx.clamp(0.0, ow), cy.clamp(0.0, oh));
    }

    // ── Step 1: element pixel → frame pixel (depends on ScaleMode) ──
    let (fx, fy) = match scale {
        ScaleMode::Fit => {
            let aspect = fw / fh;
            let elem_aspect = elem_w / elem_h;
            let (img_w, img_h, off_x, off_y) = if elem_aspect > aspect {
                // Container wider than image → letterbox on left/right
                let h = elem_h;
                let w = h * aspect;
                (w, h, (elem_w - w) / 2.0, 0.0)
            } else {
                // Container taller than image → letterbox on top/bottom
                let w = elem_w;
                let h = w / aspect;
                (w, h, 0.0, (elem_h - h) / 2.0)
            };
            (
                ((cx - off_x) / img_w * fw).clamp(0.0, fw),
                ((cy - off_y) / img_h * fh).clamp(0.0, fh),
            )
        }
        ScaleMode::Stretch => {
            // Image fills the entire container; linear mapping
            (
                (cx / elem_w * fw).clamp(0.0, fw),
                (cy / elem_h * fh).clamp(0.0, fh),
            )
        }
        ScaleMode::Original => {
            // Image shown at 1:1 pixel size (CSS object-fit: none).
            // When the image is larger than the container it is clipped;
            // when smaller it is centered with margins.
            let iw = fw.min(elem_w);
            let ih = fh.min(elem_h);
            let oxf = (elem_w - iw).max(0.0) / 2.0;
            let oyf = (elem_h - ih).max(0.0) / 2.0;
            (
                ((cx - oxf) / iw * fw).clamp(0.0, fw),
                ((cy - oyf) / ih * fh).clamp(0.0, fh),
            )
        }
    };

    // ── Step 2: frame pixel → original screen pixel ──
    // Scale up from the (possibly down-scaled) frame coords to the
    // actual remote monitor coordinates.
    let scale_x = ow / fw; // > 1.0 when host down-scaled
    let scale_y = oh / fh;
    (
        (fx * scale_x).clamp(0.0, ow),
        (fy * scale_y).clamp(0.0, oh),
    )
}

fn frame_display_style(scale: ScaleMode) -> &'static str {
    match scale {
        ScaleMode::Fit | ScaleMode::Stretch => "width:100%;height:100%;",
        ScaleMode::Original => "max-width:100%;max-height:100%;overflow:auto;",
    }
}

/// CSS style for the <img> tag that renders the remote desktop frame.
fn image_style(scale: ScaleMode) -> &'static str {
    match scale {
        // force the image to fill its container; object-fit controls rendering
        ScaleMode::Fit => {
            "width:100%;height:100%;object-fit:contain;pointer-events:none;user-select:none;"
        }
        ScaleMode::Stretch => {
            "width:100%;height:100%;object-fit:fill;pointer-events:none;user-select:none;"
        }
        ScaleMode::Original => {
            "max-width:100%;max-height:100%;object-fit:none;pointer-events:none;user-select:none;"
        }
    }
}

/// Query the desktop window's inner size in logical (CSS) pixels.
fn logical_size(window: &dioxus::desktop::DesktopService) -> (f64, f64) {
    let s = window.inner_size();
    let sf = window.scale_factor().max(frame::MIN_SCALE_FACTOR);
    ((s.width as f64) / sf, (s.height as f64) / sf)
}

// ─── Keyboard mapping ─────────────────────────────────────────────────

fn key_event_to_code(evt: &Event<KeyboardData>) -> u32 {
    // Use physical `Code` for reliable layout-independent mapping.
    // Codes must match peer::map_key_code expectations (ASCII / VK-style).
    match evt.data().code() {
        Code::Backspace => frame::VK_BACK,
        Code::Tab => frame::VK_TAB,
        Code::Enter | Code::NumpadEnter => frame::VK_RETURN,
        Code::Escape => frame::VK_ESCAPE,
        Code::Space => frame::VK_SPACE,
        Code::Delete => frame::VK_DELETE,
        Code::Insert => frame::VK_INSERT,
        Code::Home => frame::VK_HOME,
        Code::End => frame::VK_END,
        Code::PageUp => frame::VK_PRIOR,
        Code::PageDown => frame::VK_NEXT,
        Code::ArrowLeft => frame::VK_LEFT,
        Code::ArrowUp => frame::VK_UP,
        Code::ArrowRight => frame::VK_RIGHT,
        Code::ArrowDown => frame::VK_DOWN,
        Code::ShiftLeft | Code::ShiftRight => frame::VK_SHIFT,
        Code::ControlLeft | Code::ControlRight => frame::VK_CONTROL,
        Code::AltLeft | Code::AltRight => frame::VK_ALT,
        Code::MetaLeft | Code::MetaRight => frame::VK_META,
        Code::CapsLock => frame::VK_CAPITAL,
        Code::NumLock => frame::VK_NUMLOCK,
        Code::ScrollLock => frame::VK_SCROLL,
        Code::Pause => frame::VK_PAUSE,
        Code::PrintScreen => frame::VK_PRINT,
        Code::F1 => frame::VK_F1,
        Code::F2 => frame::VK_F2,
        Code::F3 => frame::VK_F3,
        Code::F4 => frame::VK_F4,
        Code::F5 => frame::VK_F5,
        Code::F6 => frame::VK_F6,
        Code::F7 => frame::VK_F7,
        Code::F8 => frame::VK_F8,
        Code::F9 => frame::VK_F9,
        Code::F10 => frame::VK_F10,
        Code::F11 => frame::VK_F11,
        Code::F12 => frame::VK_F12,
        _ => {
            match evt.data().key() {
                Key::Character(ref ch) if !ch.is_empty() => ch.chars().next().unwrap() as u32,
                _ => 0,
            }
        }
    }
}

// ─── Local keyboard shortcut handlers ─────────────────────────────────

/// Process keydown events for local shortcuts (RightCtrl+F for fullscreen).
/// Returns `true` if the event was consumed locally and should NOT be forwarded.
fn handle_keydown_local(
    evt: &Event<KeyboardData>,
    mut is_fullscreen: Signal<bool>,
    mut right_ctrl: Signal<bool>,
    window: &dioxus::desktop::DesktopService,
) -> bool {
    let key = evt.data().key();
    let code = evt.data().code();

    if key == Key::Control && code == Code::ControlRight {
        right_ctrl.set(true);
        return true;
    }

    if code == Code::KeyF
        && evt.data().modifiers().ctrl()
        && right_ctrl()
        && !evt.data().is_auto_repeating()
    {
        let fs = !is_fullscreen();
        is_fullscreen.set(fs);
        info!("[GUI] toggle fullscreen (RightCtrl+F) -> {}", fs);
        apply_fullscreen(window, fs);
        return true;
    }

    false
}

/// Process keyup events for local shortcut tracking.
/// Returns `true` if the event was consumed locally.
fn handle_keyup_local(evt: &Event<KeyboardData>, mut right_ctrl: Signal<bool>) -> bool {
    if evt.data().key() == Key::Control && evt.data().code() == Code::ControlRight {
        right_ctrl.set(false);
        return true;
    }
    false
}

// ─── Clipboard helpers ────────────────────────────────────────────────

fn get_system_clipboard() -> Result<String, String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("{e}"))?
        .get_text()
        .map_err(|e| format!("{e}"))
}

fn set_system_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("{e}"))?
        .set_text(text)
        .map_err(|e| format!("{e}"))
}

fn text_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn build_clipboard_msg(text: &str) -> Vec<u8> {
    let mut v = vec![frame::CLIP_TAG_TEXT];
    v.extend_from_slice(text.as_bytes());
    v
}

// ─── Config persistence ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppConfig {
    #[serde(default)]
    server_addr: String,
    #[serde(default)]
    remote_id: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_addr: String::new(),
            remote_id: String::new(),
        }
    }
}

fn config_path() -> std::path::PathBuf {
    let base = dirs().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("rtcdesk.json")
}

fn dirs() -> Option<std::path::PathBuf> {
    std::env::var("RTCDESK_CONFIG_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| dirs_next().map(|d| d.join("rtcdesk")))
}

fn dirs_next() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| std::path::PathBuf::from(h).join(".config"))
            })
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(|h| {
            std::path::PathBuf::from(h)
                .join("Library")
                .join("Application Support")
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(std::path::PathBuf::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn load_config() -> AppConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

fn save_config(cfg: &AppConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("[GUI] Failed to create config dir {}: {}", parent.display(), e);
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        if let Err(e) = std::fs::write(&path, json) {
            warn!("[GUI] Failed to save config to {}: {}", path.display(), e);
        }
    }
}

// ─── Utilities ────────────────────────────────────────────────────────

fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

fn load_stun_servers() -> Vec<String> {
    let raw = std::env::var("RTCDESK_STUN").unwrap_or_default();
    let v: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if v.is_empty() {
        vec![frame::DEFAULT_STUN.into()]
    } else {
        v
    }
}
