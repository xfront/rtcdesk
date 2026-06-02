//! Async viewer event loop: connects to the signaling server and drives
//! all RTC traffic (frames, input, clipboard, quality changes, disconnect).

use std::cmp;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use dioxus::signals::WritableExt;
use log::{error, info, warn};
use tokio::sync::mpsc;
use dioxus::signals::Signal;

use shared::frame;
use crate::config::{load_config, load_stun_servers, add_history_entry};
use crate::local_id::rand_id;
use crate::quality::QUALITY_PRESETS;
use crate::remote::view::SessionState;
use crate::viewer::{RtcViewer, ViewerCallback};
use crate::Page;
// ─── Connection statistics ───────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
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

// ─── File send command ─────────────────────────────────────────────────────

pub enum FileSendCmd {
    Send { paths: Vec<std::path::PathBuf> },
}

// ─── Clipboard file sync state (echo prevention) ──────────────────────────
//
// When we receive file URIs from the host and write them to the local
// clipboard, the next clip_poll will detect those same URIs and try to
// send them back.  We record the hash of the URIs we just received so
// the writer can suppress the echo — same pattern as the host side
// CLIP_SYNC.
//
// Image sync uses the same pattern: we hash the RGBA bytes received from
// the host so clip_poll can avoid echoing it back.

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

fn is_received_image_echo(rgba: &[u8]) -> bool {
    match CLIP_IMAGE_SYNC.lock() {
        Ok(sync) => bytes_hash(rgba) == sync.last_received_hash,
        Err(_) => false,
    }
}

// ─── GUI callback ─────────────────────────────────────────────────────────────

pub struct GuiCallback {
    pub frame_tx:     mpsc::UnboundedSender<(Vec<u8>, u32, u32, u32, u32)>,
    pub state_tx:     mpsc::UnboundedSender<SessionState>,
    pub clipboard_tx: mpsc::UnboundedSender<String>,
    #[allow(dead_code)]
    pub stats_tx:     mpsc::UnboundedSender<ConnectionStats>,
}

impl ViewerCallback for GuiCallback {
    fn on_connected(&self) {
        let _ = self.state_tx.send(SessionState::Connected);
    }
    fn on_disconnected(&self) {
        let _ = self.state_tx.send(SessionState::Error("连接已断开".into()));
    }
    fn on_frame(
        &self,
        data: Vec<u8>,
        orig_w: u32,
        orig_h: u32,
        frame_w: u32,
        frame_h: u32,
    ) {
        let _ = self.frame_tx.send((data, orig_w, orig_h, frame_w, frame_h));
    }
    fn on_connection_state(&self, state: String) {
        info!("[GUI] state: {}", state);
    }
    fn on_error(&self, message: String) {
        error!("[GUI] {}", message);
        let _ = self.state_tx.send(SessionState::Error(message));
    }
    fn on_clipboard_text(&self, text: String) {
        let _ = self.clipboard_tx.send(text);
    }
    fn on_clipboard_file_uris(&self, uris: String) {
        info!("[GUI] clipboard files received: {}", uris);
        // Set the file URIs in the local clipboard so that the user can
        // Ctrl+V in a local file browser.
        if let Err(e) = crate::clipboard::set_clipboard_file_uris(&uris) {
            warn!("[GUI] set_clipboard_file_uris error: {}", e);
        }
        // Record the URIs hash for echo prevention in clip_poll.
        record_received_file_uris(&uris);
    }
    fn on_clipboard_image(&self, rgba: Vec<u8>, width: u32, height: u32) {
        info!("[GUI] clipboard image received: {}x{} ({} bytes)", width, height, rgba.len());
        if let Err(e) = crate::clipboard::set_clipboard_image(&rgba, width, height) {
            warn!("[GUI] set_clipboard_image error: {}", e);
        }
        // Record hash for echo prevention in clip_poll.
        record_received_image(&rgba);
    }
    fn on_stats_update(&self, stats: ConnectionStats) {
        let _ = self.stats_tx.send(stats);
    }
}

// ─── ViewerState (shared signal bundle) ──────────────────────────────────────

pub struct ViewerState {
    pub frame_b64:        Signal<String>,
    pub orig_w:           Signal<u32>,
    pub orig_h:           Signal<u32>,
    pub frame_w:          Signal<u32>,
    pub frame_h:          Signal<u32>,
    pub state:            Signal<SessionState>,
    pub local_clipboard:  Signal<String>,
    pub clipboard_synced: Signal<bool>,
    pub page:             Signal<Page>,
    pub view_only:        Signal<bool>,
    #[allow(dead_code)]
    pub is_fullscreen:    Signal<bool>,
    pub window_cmd:       Signal<Vec<crate::WindowCmd>>,
    pub stats:            Signal<ConnectionStats>,
    pub file_transfer:    Signal<crate::remote::file_transfer::FileTransferState>,
    #[allow(dead_code)]
    pub toast:            Signal<Option<crate::toast::ToastMessage>>,
}

// ─── File chunking & sending helper ───────────────────────────────────────
//
// Shared by both the file_dialog path (FileSendCmd) and the clipboard
// sync path (Ctrl+C file → send to host).  Returns Ok(sent_bytes) on
// success, or Err(message) on failure.

async fn send_files_via_clipboard(
    viewer: &RtcViewer,
    paths: &[std::path::PathBuf],
) -> Result<u64, String> {
    // Build file list header: [0x03][count:u16le] + per entry: [name_len:u16le][name:utf8][size:u64le]
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

    // Send file list header
    viewer.send_clipboard_data(list_buf).await
        .map_err(|e| format!("发送文件列表失败: {}", e))?;

    // Send file chunks
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

            viewer.send_clipboard_data(chunk).await
                .map_err(|e| format!("发送文件数据失败: {}", e))?;

            offset += len as u64;
            sent_bytes += len as u64;

            // Inter-chunk delay to avoid flooding
            tokio::time::sleep(Duration::from_millis(frame::INTER_CHUNK_DELAY_MS)).await;
        }
    }

    // Send completion marker
    viewer.send_clipboard_data(vec![frame::CLIP_TAG_DONE]).await
        .map_err(|e| format!("发送完成标记失败: {}", e))?;

    Ok(sent_bytes)
}

// ─── Async event loop ────────────────────────────────────────────────────────

pub async fn run_viewer(
    srv: String,
    rid: String,
    local_id: String,
    qidx: usize,
    password: String,
    mut vs: ViewerState,
    mut frame_rx:     mpsc::UnboundedReceiver<(Vec<u8>, u32, u32, u32, u32)>,
    mut state_rx:     mpsc::UnboundedReceiver<SessionState>,
    mut clip_rx:      mpsc::UnboundedReceiver<String>,
    mut input_rx:     mpsc::UnboundedReceiver<InputCmd>,
    mut qt_rx:        mpsc::UnboundedReceiver<usize>,
    mut dc_rx:        mpsc::UnboundedReceiver<()>,
    mut clip_send_rx: mpsc::UnboundedReceiver<String>,
    mut stats_rx:     mpsc::UnboundedReceiver<ConnectionStats>,
    mut file_send_rx: mpsc::UnboundedReceiver<FileSendCmd>,
    state_tx_clone:   mpsc::UnboundedSender<SessionState>,
    cb: Arc<GuiCallback>,
) {
    let view_only = vs.view_only;
    let frame_w = vs.frame_w;
    let frame_h = vs.frame_h;

    let qidx_safe = qidx.min(QUALITY_PRESETS.len().saturating_sub(1));
    let viewer_id = if local_id.is_empty() {
        format!("gui-{}", rand_id())
    } else {
        local_id
    };

    let mut viewer = RtcViewer::new(viewer_id, rid.clone());
    viewer.set_callback(cb);
    viewer.set_pic_quality(QUALITY_PRESETS[qidx_safe].0);
    viewer.set_resolution(QUALITY_PRESETS[qidx_safe].2, QUALITY_PRESETS[qidx_safe].3);
    viewer.set_fps(QUALITY_PRESETS[qidx_safe].4);

    let stun = load_stun_servers();
    if let Err(e) = viewer.connect(srv.clone(), stun, password).await {
        let _ = state_tx_clone.send(SessionState::Error(format!("连接失败: {}", e)));
        return;
    }

    let mut last_clip_sent_hash: u64 = 0;
    let mut last_file_uris_sent: String = String::new();
    let mut last_image_sent_hash: u64 = 0;
    let mut clip_poll =
        tokio::time::interval(Duration::from_millis(frame::CLIPBOARD_WRITE_POLL_MS));
    clip_poll.tick().await;

    // FPS 计数器（用于统计信息）
    let mut frame_count: u32 = 0;
    let mut stats_timer = tokio::time::interval(Duration::from_millis(frame::STATS_REFRESH_MS));
    stats_timer.tick().await;
    let mut current_bytes_mb: f64 = 0.0;

    loop {
        tokio::select! {
            Some((data, ow, oh, w, h)) = frame_rx.recv() => {
                current_bytes_mb += data.len() as f64 / (1024.0 * 1024.0);
                frame_count += 1;
                vs.frame_b64.set(format!("data:image/jpeg;base64,{}", BASE64.encode(&data)));
                vs.orig_w.set(ow); vs.orig_h.set(oh);
                vs.frame_w.set(w); vs.frame_h.set(h);
            }
            Some(s) = state_rx.recv() => {
                if matches!(s, SessionState::Connected) {
                    let mut cfg = load_config();
                    add_history_entry(&mut cfg, &rid, &srv, "");
                }
                vs.state.set(s);
            }
            Some(text) = clip_rx.recv() => {
                vs.local_clipboard.set(text.clone());
                vs.clipboard_synced.set(true);
                last_clip_sent_hash = text_hash(&text);
                if let Err(e) = crate::clipboard::set_system_clipboard(&text) {
                    warn!("[Gui] set_system_clipboard error: {}", e);
                }
            }
            Some(text) = clip_send_rx.recv() => {
                if let Err(e) = viewer.send_clipboard_data(build_clipboard_msg(&text)).await {
                    error!("[Gui] send_clipboard_data error: {}", e);
                }
                last_clip_sent_hash = text_hash(&text);
            }
            Some(stats) = stats_rx.recv() => {
                vs.stats.set(stats);
            }
            Some(cmd) = file_send_rx.recv() => {
                match cmd {
                    FileSendCmd::Send { paths } => {
                        let file_names: Vec<String> = paths.iter()
                            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                            .collect();
                        let total_files = file_names.len();

                        // Set state to Transferring
                        vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Transferring {
                            files: file_names.clone(),
                            current_idx: 0,
                            progress: 0.0,
                        });

                        // Calculate total bytes for progress tracking
                        let total_bytes: u64 = paths.iter()
                            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
                            .sum();

                        // Send file list header
                        let mut list_buf = Vec::with_capacity(frame::CLIP_FILE_LIST_BUF);
                        list_buf.push(frame::CLIP_TAG_FILE_LIST);
                        let count = paths.len() as u16;
                        list_buf.extend_from_slice(&count.to_le_bytes());
                        for p in &paths {
                            let name = p.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| format!("file_{}", list_buf.len()));
                            let name_bytes = name.as_bytes();
                            list_buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                            list_buf.extend_from_slice(name_bytes);
                            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                            list_buf.extend_from_slice(&size.to_le_bytes());
                        }

                        if let Err(e) = viewer.send_clipboard_data(list_buf).await {
                            error!("[Gui] send file list error: {}", e);
                            vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Error {
                                message: format!("发送文件列表失败: {}", e),
                            });
                            continue;
                        }

                        // Send file chunks one by one
                        let mut sent_bytes: u64 = 0;
                        let mut transfer_error = None;
                        for (i, p) in paths.iter().enumerate() {
                            // Update current file index
                            {
                                let file_transfer = vs.file_transfer;
                                let cur = file_transfer();
                                if let crate::remote::file_transfer::FileTransferState::Transferring { files, .. } = &cur {
                                    vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Transferring {
                                        files: files.clone(),
                                        current_idx: i,
                                        progress: if total_bytes > 0 { sent_bytes as f64 / total_bytes as f64 } else { 0.0 },
                                    });
                                }
                            }

                            let data = match std::fs::read(p) {
                                Ok(d) => d,
                                Err(e) => {
                                    error!("[Gui] Cannot read file {}: {}", p.display(), e);
                                    transfer_error = Some(format!(
                                        "读取文件 {} 失败: {}",
                                        p.file_name().unwrap_or_default().to_string_lossy(),
                                        e
                                    ));
                                    break;
                                }
                            };

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

                                if let Err(e) = viewer.send_clipboard_data(chunk).await {
                                    error!("[Gui] send file chunk error: {}", e);
                                    transfer_error = Some(format!("发送文件数据失败: {}", e));
                                    break;
                                }

                                offset += len as u64;
                                sent_bytes += len as u64;

                                // Update progress
                                let progress = if total_bytes > 0 { sent_bytes as f64 / total_bytes as f64 } else { 0.0 };
                                {
                                    let file_transfer = vs.file_transfer;
                                    let cur = file_transfer();
                                    if let crate::remote::file_transfer::FileTransferState::Transferring { files, .. } = &cur {
                                        vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Transferring {
                                            files: files.clone(),
                                            current_idx: i,
                                            progress,
                                        });
                                    }
                                }

                                // Inter-chunk delay to avoid flooding
                                tokio::time::sleep(Duration::from_millis(frame::INTER_CHUNK_DELAY_MS)).await;
                            }

                            if transfer_error.is_some() {
                                break;
                            }
                        }

                        // Send completion marker (only if no error)
                        if transfer_error.is_none() {
                            if let Err(e) = viewer.send_clipboard_data(vec![frame::CLIP_TAG_DONE]).await {
                                error!("[Gui] send file done marker error: {}", e);
                            }
                            vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Completed {
                                count: total_files,
                            });
                        } else {
                            vs.file_transfer.set(crate::remote::file_transfer::FileTransferState::Error {
                                message: transfer_error.unwrap(),
                            });
                        }
                    }
                }
            }
            Some(cmd) = input_rx.recv() => {
                // 只看模式：跳过所有输入转发
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
                vs.window_cmd.write().push(crate::WindowCmd::ExitFullscreen);
                vs.page.set(Page::Main);
                return;
            }
            _ = stats_timer.tick() => {
                // 每秒更新一次连接统计
                let fps = frame_count;
                frame_count = 0;
                {
                    let mut s = vs.stats.write();
                    s.fps = fps;
                    s.bytes_transferred_mb = current_bytes_mb;
                    s.resolution_w = frame_w();
                    s.resolution_h = frame_h();
                }
            }
            _ = clip_poll.tick() => {
                if view_only() {
                    continue;
                }
                // ── Text clipboard sync ─────────────
                if let Ok(text) = crate::clipboard::get_system_clipboard() {
                    let h = text_hash(&text);
                    if !text.is_empty() && h != last_clip_sent_hash {
                        last_clip_sent_hash = h;
                        if let Err(e) =
                            viewer.send_clipboard_data(build_clipboard_msg(&text)).await
                        {
                            warn!("[Gui] auto clip sync error: {}", e);
                        }
                    }
                }
                // ── File URI clipboard sync ─────────
                // When the user Ctrl+C a file in the local file browser,
                // the clipboard contains text/uri-list with file:// URIs.
                // We detect these, read the files, and send them to the
                // host via the clipboard DC.  The host then saves the
                // files and simulates Ctrl+V in the remote desktop.
                if let Some(uris) = crate::clipboard::get_clipboard_file_uris() {
                    if !uris.is_empty() && uris != last_file_uris_sent
                        && !is_received_file_uris_echo(&uris)
                    {
                        let paths = crate::clipboard::parse_file_uris(&uris);
                        if !paths.is_empty() {
                            info!("[Gui] Clipboard file URIs detected ({} files), sending to host", paths.len());
                            match send_files_via_clipboard(&viewer, &paths).await {
                                Ok(bytes) => {
                                    info!("[Gui] Clipboard files sent: {} bytes", bytes);
                                    last_file_uris_sent = uris;
                                }
                                Err(e) => {
                                    warn!("[Gui] Clipboard file send error: {}", e);
                                }
                            }
                        }
                    }
                }
                // ── Image clipboard sync ────────────
                // When the user copies an image (e.g. screenshot, browser image),
                // detect it and push RGBA pixels to the host wrapped in
                // CLIP_TAG_IMAGE so the host can populate its own clipboard.
                if let Some(img) = crate::clipboard::get_clipboard_image() {
                    let h = bytes_hash(&img.rgba);
                    if h != last_image_sent_hash && !is_received_image_echo(&img.rgba) {
                        if img.rgba.len() <= shared::frame::CLIP_IMAGE_MAX_BYTES {
                            let msg = build_image_clipboard_msg(&img.rgba, img.width, img.height);
                            match viewer.send_clipboard_data(msg).await {
                                Ok(()) => {
                                    info!("[Gui] Clipboard image sent: {}x{} ({} bytes)",
                                          img.width, img.height, img.rgba.len());
                                    last_image_sent_hash = h;
                                }
                                Err(e) => {
                                    warn!("[Gui] Clipboard image send error: {}", e);
                                }
                            }
                        } else {
                            warn!("[Gui] Clipboard image too large ({} bytes), skipping",
                                  img.rgba.len());
                            last_image_sent_hash = h; // don't retry same large image
                        }
                    }
                }
            }
            else => break,
        }
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────

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

fn build_clipboard_msg(text: &str) -> Vec<u8> {
    let mut v = vec![frame::CLIP_TAG_TEXT];
    v.extend_from_slice(text.as_bytes());
    v
}

/// Build a clipboard image message:
/// [CLIP_TAG_IMAGE:1][width:u32le][height:u32le][rgba_data...]
fn build_image_clipboard_msg(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + 4 + 4 + rgba.len());
    v.push(frame::CLIP_TAG_IMAGE);
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&height.to_le_bytes());
    v.extend_from_slice(rgba);
    v
}
