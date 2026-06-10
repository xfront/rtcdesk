//! Async viewer event loop: connects to the signaling server and
//! bridges all WebRTC operations entirely in Rust.
//!
//! Video is received via webrtc-rs video track → H264 decode → JPEG.
//! Input/clipboard DataChannels are managed in Rust.
//! No JavaScript bridge is used.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use dioxus::signals::WritableExt;
use dioxus::signals::Signal;
use log::{error, info, warn};
use tokio::sync::mpsc;

use shared::frame;
use shared::streaming;
use crate::config::{load_config, load_stun_servers, add_history_entry};
use crate::local_id::rand_id;
use crate::quality::QUALITY_PRESETS;
use crate::remote::view::SessionState;
use crate::viewer::{RtcViewer, ViewerCallback};
use crate::Page;

// ─── Connection statistics ───────────────────────────────────────────────

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ConnectionStats {
    pub fps: u32,
    pub rtt_ms: u32,
    pub bytes_transferred_mb: f64,
    pub resolution_w: u32,
    pub resolution_h: u32,
}

// ─── Input command ────────────────────────────────────────────────────────────

pub enum InputCmd {
    Mouse { x: f64, y: f64, button: u8, pressed: bool },
    Key   { key_code: u32, pressed: bool },
    Wheel { dx: i32, dy: i32 },
}

// ─── File send command ────────────────────────────────────────────────────

pub enum FileSendCmd {
    Send { paths: Vec<std::path::PathBuf> },
}

// ─── Clipboard file sync state (echo prevention) ──────────────────────────

use std::sync::Mutex;

struct ClipFileSync {
    last_received_hash: u64,
}

struct ClipImageSync {
    last_received_hash: u64,
}

static CLIP_FILE_SYNC: Mutex<ClipFileSync> = Mutex::new(ClipFileSync {
    last_received_hash: 0,
});

static CLIP_IMAGE_SYNC: Mutex<ClipImageSync> = Mutex::new(ClipImageSync {
    last_received_hash: 0,
});

fn record_received_file_uris(uris: &str) {
    if let Ok(mut sync) = CLIP_FILE_SYNC.lock() {
        sync.last_received_hash = text_hash(uris);
    }
}

fn is_received_file_uris_echo(uris: &str) -> bool {
    match CLIP_FILE_SYNC.lock() {
        Ok(sync) => text_hash(uris) == sync.last_received_hash,
        Err(_) => false,
    }
}

fn record_received_image(rgba: &[u8]) {
    if let Ok(mut sync) = CLIP_IMAGE_SYNC.lock() {
        sync.last_received_hash = bytes_hash(rgba);
    }
}

#[allow(dead_code)]
fn is_received_image_echo(rgba: &[u8]) -> bool {
    match CLIP_IMAGE_SYNC.lock() {
        Ok(sync) => bytes_hash(rgba) == sync.last_received_hash,
        Err(_) => false,
    }
}

// ─── GUI callback ─────────────────────────────────────────────────────────

pub struct GuiCallback {
    pub frame_tx:         mpsc::UnboundedSender<(u32, u32)>,
    pub state_tx:         mpsc::UnboundedSender<SessionState>,
    pub clipboard_tx:     mpsc::UnboundedSender<String>,
    #[allow(dead_code)]
    pub stats_tx:         mpsc::UnboundedSender<ConnectionStats>,
}

impl ViewerCallback for GuiCallback {
    fn on_connected(&self) {
        let _ = self.state_tx.send(SessionState::Connected);
    }
    fn on_disconnected(&self) {
        let _ = self.state_tx.send(SessionState::Error("连接已断开".into()));
    }
    fn on_connection_state(&self, state: String) {
        info!("[GUI] state: {}", state);
    }
    fn on_error(&self, message: String) {
        error!("[GUI] {}", message);
        let _ = self.state_tx.send(SessionState::Error(message));
    }
    fn on_video_frame(&self, bgra: &[u8], width: u32, height: u32) {
        // Forward frame dimensions to the UI (pixel data is in FrameSlot)
        let frame_slot = crate::get_frame_slot();
        if let Some(ref slot) = frame_slot {
            let mut slot = slot.lock().unwrap();
            slot.update(&bgra, width, height);
        }

        let _ = self.frame_tx.send((width, height));
    }
    fn on_clipboard_text(&self, text: String) {
        let _ = self.clipboard_tx.send(text);
    }
    fn on_clipboard_file_uris(&self, uris: String) {
        info!("[GUI] clipboard files received: {}", uris);
        if let Err(e) = crate::clipboard::set_clipboard_file_uris(&uris) {
            warn!("[GUI] set_clipboard_file_uris error: {}", e);
        }
        record_received_file_uris(&uris);
    }
    fn on_clipboard_image(&self, rgba: Vec<u8>, width: u32, height: u32) {
        info!("[GUI] clipboard image received: {}x{} ({} bytes)", width, height, rgba.len());
        if let Err(e) = crate::clipboard::set_clipboard_image(&rgba, width, height) {
            warn!("[GUI] set_clipboard_image error: {}", e);
        }
        record_received_image(&rgba);
    }
    fn on_data_channel_open(&self, label: String) {
        info!("[GUI] DataChannel opened: {}", label);
    }
}

// ─── ViewerState (shared signal bundle) ──────────────────────────────────

pub struct ViewerState {
    pub frame_seq:        Signal<u64>,
    pub frame_w:          Signal<u32>,
    pub frame_h:          Signal<u32>,
    pub state:            Signal<SessionState>,
    pub local_clipboard:  Signal<String>,
    pub clipboard_synced: Signal<bool>,
    pub page:             Signal<Page>,
    pub view_only:        Signal<bool>,
    pub window_cmd:       Signal<Vec<crate::WindowCmd>>,
    pub stats:            Signal<ConnectionStats>,
    pub file_transfer:    Signal<crate::remote::file_transfer::FileTransferState>,
    #[allow(dead_code)]
    pub toast:            Signal<Option<crate::toast::ToastMessage>>,
}

// ─── Async event loop ────────────────────────────────────────────────────

pub async fn run_viewer(
    srv: String,
    rid: String,
    local_id: String,
    qidx: usize,
    password: String,
    mut vs: ViewerState,
    mut frame_rx:     mpsc::UnboundedReceiver<(u32, u32)>,
    mut state_rx:       mpsc::UnboundedReceiver<SessionState>,
    mut clip_rx:        mpsc::UnboundedReceiver<String>,
    mut input_rx:       mpsc::UnboundedReceiver<InputCmd>,
    mut qt_rx:          mpsc::UnboundedReceiver<usize>,
    mut dc_rx:          mpsc::UnboundedReceiver<()>,
    mut _clip_send_rx:  mpsc::UnboundedReceiver<String>,
    mut _stats_rx:      mpsc::UnboundedReceiver<ConnectionStats>,
    mut _file_send_rx:  mpsc::UnboundedReceiver<FileSendCmd>,
    state_tx_clone:     mpsc::UnboundedSender<SessionState>,
    cb: Arc<GuiCallback>,
) {
    let view_only = vs.view_only;
    let qidx_safe = qidx.min(QUALITY_PRESETS.len().saturating_sub(1));

    #[allow(unused_assignments)]
    let mut reconnect_attempts = 0u32;

    'reconnect: loop {
        let viewer_id = if local_id.is_empty() {
            format!("gui-{}", rand_id())
        } else {
            local_id.clone()
        };

        let mut viewer = RtcViewer::new(viewer_id.clone(), rid.clone(), cb.clone());
        viewer.set_pic_quality(QUALITY_PRESETS[qidx_safe].0);
        viewer.set_resolution(QUALITY_PRESETS[qidx_safe].2, QUALITY_PRESETS[qidx_safe].3);
        viewer.set_fps(QUALITY_PRESETS[qidx_safe].4);

        let stun = load_stun_servers();
        match viewer.connect(srv.clone(), stun.clone(), password.clone()).await {
            Ok(()) => {
                reconnect_attempts = 0;
                info!("Connected to host '{}'", rid);
            }
            Err(e) => {
                let _ = state_tx_clone.send(SessionState::Error(format!("连接失败: {}", e)));
                break 'reconnect;
            }
        }

        let mut last_clip_sent_hash: u64 = 0;
        let mut last_file_uris_sent: String = String::new();
        let mut last_image_sent_hash: u64 = 0;
        let mut clip_poll =
            tokio::time::interval(Duration::from_millis(frame::CLIPBOARD_WRITE_POLL_MS));
        clip_poll.tick().await;

        let mut stats_timer =
            tokio::time::interval(Duration::from_millis(frame::STATS_REFRESH_MS));
        stats_timer.tick().await;

        let mut disconnected = false;
        let mut frame_count: u32 = 0;
        let mut stats_start = std::time::Instant::now();

        loop {
            tokio::select! {
                Some(s) = state_rx.recv() => {
                    match &s {
                        SessionState::Connected => {
                            let mut cfg = load_config();
                            add_history_entry(&mut cfg, &rid, &srv, "");
                        }
                        SessionState::Error(_) => {
                            disconnected = true;
                        }
                        _ => {}
                    }
                    vs.state.set(s);
                }
                Some((width, height)) = frame_rx.recv() => {
                    // Read sequence from FrameSlot (pixel data already written by decode thread)
                    let seq = crate::get_frame_slot()
                        .map(|s| s.lock().unwrap().sequence)
                        .unwrap_or(0);
                    vs.frame_seq.set(seq);
                    vs.frame_w.set(width); vs.frame_h.set(height);
                    frame_count += 1;
                    if frame_count % 30 == 1 {
                        info!(
                            "UI frame #{}: {}x{} seq={}",
                            frame_count, width, height, seq
                        );
                    }
                }
                Some(text) = clip_rx.recv() => {
                    vs.local_clipboard.set(text.clone());
                    vs.clipboard_synced.set(true);
                    last_clip_sent_hash = text_hash(&text);
                    if let Err(e) = crate::clipboard::set_system_clipboard(&text) {
                        warn!("[Gui] set_system_clipboard error: {}", e);
                    }
                }
                Some(cmd) = input_rx.recv() => {
                    // View-only mode: skip all input forwarding
                    if view_only() {
                        continue;
                    }
                    match cmd {
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
                    }
                }
                Some(q) = qt_rx.recv() => {
                    let qsafe = q.min(QUALITY_PRESETS.len().saturating_sub(1));
                    let qref = &QUALITY_PRESETS[qsafe];
                    viewer.set_pic_quality(qref.0);
                    viewer.set_resolution(qref.2, qref.3);
                    viewer.set_fps(qref.4);
                    let max_dim = std::cmp::max(qref.2, qref.3);
                    if let Err(e) = viewer.send_quality_update(qref.0, max_dim, qref.4, qref.5).await {
                        error!("[Gui] send_quality_update error: {}", e);
                    }
                }
                Some(()) = dc_rx.recv() => {
                    viewer.disconnect().await;
                    vs.window_cmd.write().push(crate::WindowCmd::ExitFullscreen);
                    vs.page.set(Page::Main);
                    break 'reconnect;
                }
                _ = stats_timer.tick() => {
                    let elapsed = stats_start.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        let fps = (frame_count as f64 / elapsed) as u32;
                        let stats = vs.stats;
                        let mut stats = stats();
                        stats.fps = fps;
                        vs.stats.set(stats);
                    }
                    if stats_start.elapsed() >= Duration::from_secs(3) {
                        frame_count = 0;
                        stats_start = std::time::Instant::now();
                    }
                }
                _ = clip_poll.tick() => {
                    if view_only() { continue; }
                    // Poll system clipboard and send to host via Rust DataChannel
                    if let Ok(text) = crate::clipboard::get_system_clipboard() {
                        let h = text_hash(&text);
                        if !text.is_empty() && h != last_clip_sent_hash {
                            last_clip_sent_hash = h;
                            let mut msg = vec![frame::CLIP_TAG_TEXT];
                            msg.extend_from_slice(text.as_bytes());
                            let _ = viewer.send_clipboard(&msg).await;
                        }
                    }
                    // File URIs clipboard
                    if let Some(uris) = crate::clipboard::get_clipboard_file_uris() {
                        if !uris.is_empty() && uris != last_file_uris_sent
                            && !is_received_file_uris_echo(&uris)
                        {
                            let paths = crate::clipboard::parse_file_uris(&uris);
                            if !paths.is_empty() {
                                info!("[Gui] Clipboard file URIs detected ({} files), sending to host", paths.len());
                                // Send files via clipboard DataChannel
                                if let Ok(sent) = send_files_via_clipboard(&viewer, &paths).await {
                                    info!("[Gui] Clipboard files sent: {} bytes", sent);
                                    last_file_uris_sent = uris;
                                }
                            }
                        }
                    }
                    // Image clipboard
                    if let Some(img) = crate::clipboard::get_clipboard_image() {
                        let h = bytes_hash(&img.rgba);
                        if h != last_image_sent_hash && !is_received_image_echo(&img.rgba) {
                            if img.rgba.len() <= shared::frame::CLIP_IMAGE_MAX_BYTES {
                                let mut msg = Vec::with_capacity(1 + 8 + img.rgba.len());
                                msg.push(frame::CLIP_TAG_IMAGE);
                                msg.extend_from_slice(&(img.width as u32).to_le_bytes());
                                msg.extend_from_slice(&(img.height as u32).to_le_bytes());
                                msg.extend_from_slice(&img.rgba);
                                if viewer.send_clipboard(&msg).await.is_ok() {
                                    info!("[Gui] Clipboard image sent: {}x{}", img.width, img.height);
                                    last_image_sent_hash = h;
                                }
                            } else {
                                warn!("[Gui] Clipboard image too large, skipping");
                                last_image_sent_hash = h;
                            }
                        }
                    }
                }
                else => break,
            }

            if disconnected {
                info!(
                    "Disconnected from host '{}' — reconnect attempt {}/{}",
                    rid,
                    reconnect_attempts + 1,
                    streaming::RECONNECT_MAX_ATTEMPTS,
                );

                if reconnect_attempts >= streaming::RECONNECT_MAX_ATTEMPTS {
                    error!("Max reconnect attempts reached, giving up");
                    break 'reconnect;
                }

                reconnect_attempts += 1;
                let delay = std::cmp::min(
                    streaming::RECONNECT_BASE_DELAY_MS
                        * streaming::RECONNECT_BACKOFF_FACTOR.pow(reconnect_attempts - 1) as u64,
                    streaming::RECONNECT_MAX_DELAY_MS,
                );
                info!("Reconnecting in {} ms...", delay);
                vs.state.set(SessionState::Error(format!(
                    "连接已断开，{}ms 后自动重连 ({}/{})",
                    delay,
                    reconnect_attempts,
                    streaming::RECONNECT_MAX_ATTEMPTS,
                )));
                tokio::time::sleep(Duration::from_millis(delay)).await;
                continue 'reconnect;
            }
        }

        break 'reconnect;
    }
}

// ─── File chunking & sending helper ──────────────────────────────────────

async fn send_files_via_clipboard(
    viewer: &RtcViewer,
    paths: &[std::path::PathBuf],
) -> Result<u64, String> {
    // Build file list header
    let mut list_buf = Vec::with_capacity(frame::CLIP_FILE_LIST_BUF);
    list_buf.push(frame::CLIP_TAG_FILE_LIST);
    let count = paths.len() as u16;
    list_buf.extend_from_slice(&count.to_le_bytes());
    for p in paths {
        let name = p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file_{}", list_buf.len()));
        let name_bytes = name.as_bytes();
        list_buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        list_buf.extend_from_slice(name_bytes);
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        list_buf.extend_from_slice(&size.to_le_bytes());
    }
    viewer.send_clipboard(&list_buf).await.map_err(|e| format!("send list error: {}", e))?;

    let mut sent_bytes: u64 = 0;
    for (i, p) in paths.iter().enumerate() {
        let data = std::fs::read(p)
            .map_err(|e| format!("读取文件 {} 失败: {}", p.display(), e))?;

        let chunk_size = frame::FILE_CHUNK_SIZE;
        let mut offset = 0u64;
        while offset < data.len() as u64 {
            let len = std::cmp::min(chunk_size, data.len() - offset as usize);
            let mut chunk = Vec::with_capacity(frame::CLIP_CHUNK_MSG_SIZE + len);
            chunk.push(frame::CLIP_TAG_FILE_CHUNK);
            chunk.extend_from_slice(&(i as u16).to_le_bytes());
            chunk.extend_from_slice(&offset.to_le_bytes());
            chunk.extend_from_slice(&(len as u32).to_le_bytes());
            chunk.extend_from_slice(&data[offset as usize..offset as usize + len]);

            viewer.send_clipboard(&chunk).await.map_err(|e| format!("send chunk error: {}", e))?;

            offset += len as u64;
            sent_bytes += len as u64;
            tokio::time::sleep(Duration::from_millis(frame::INTER_CHUNK_DELAY_MS)).await;
        }
    }

    // Send completion marker
    viewer.send_clipboard(&[frame::CLIP_TAG_DONE]).await.map_err(|e| format!("send done error: {}", e))?;

    Ok(sent_bytes)
}

// ─── Utility ─────────────────────────────────────────────────────────────

fn text_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn bytes_hash(b: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}
