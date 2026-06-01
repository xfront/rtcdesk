//! WebRTC viewer client for RTCDesk.

use anyhow::{Context, Result};
use bytes::BytesMut;
use log::{error, info, warn};
use std::cmp;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfiguration,
    RTCConfigurationBuilder, RTCIceConnectionState, RTCIceServer, RTCPeerConnectionIceEvent,
    RTCPeerConnectionState, RTCSessionDescription,
};
use shared::{frame, signal};
use shared::frame::FrameAssembler;
use shared::signal::{PeerRole, SignalMessage};

pub trait ViewerCallback: Send + Sync {
    fn on_connected(&self);
    fn on_disconnected(&self);
    /// Called when a complete desktop frame has been reassembled.
    /// `orig_w`/`orig_h` are the remote monitor's actual resolution (for mouse mapping);
    /// `frame_w`/`frame_h` are the JPEG dimensions (may differ if the host down-scaled).
    fn on_frame(&self, data: Vec<u8>, orig_w: u32, orig_h: u32, frame_w: u32, frame_h: u32);
    fn on_connection_state(&self, state: String);
    fn on_error(&self, message: String);
    /// Called when the remote host sent text clipboard data.
    fn on_clipboard_text(&self, text: String);
    /// Called when the remote host sent file URIs (newline-separated file:// URIs)
    /// after a complete file transfer.
    fn on_clipboard_file_uris(&self, uris: String);
    /// Called when the remote host sent an image clipboard payload.
    /// `rgba` is raw RGBA pixel data, `width`×`height` in pixels.
    fn on_clipboard_image(&self, _rgba: Vec<u8>, _width: u32, _height: u32) {}
    /// 周期性调用，提供更新的连接统计信息。
    fn on_stats_update(&self, _stats: crate::remote::viewer_loop::ConnectionStats) {}
}

/// Discover local IP addresses for ICE host candidates.
/// Falls back to 0.0.0.0 binding if no local IP is found.
fn discover_udp_addrs() -> Vec<String> {
    match local_ip_address::local_ip() {
        Ok(ip) => vec![format!("{}:0", ip)],
        Err(_) => vec!["0.0.0.0:0".to_string()],
    }
}

type WsSplitSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

/// Manages a single WebRTC viewer connection.
pub struct RtcViewer {
    ws_write: Arc<Mutex<Option<WsSplitSink>>>,
    pc: Option<Arc<dyn PeerConnection>>,
    viewer_id: String,
    host_id: String,
    callback: Option<Arc<dyn ViewerCallback>>,
    connected: Arc<AtomicBool>,
    input_channel: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_channel: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    pub ws_disconnected: Arc<Notify>,
    pic_quality: u32,     //  视频质量等级
    fps: u32,             // 视频帧率
    width: u32,           // 视频宽度
    height: u32,          // 视频高度
    bitrate: Option<u32>, // 视频比特率
}

impl RtcViewer {
    pub fn new(viewer_id: String, host_id: String) -> Self {
        Self {
            ws_write: Arc::new(Mutex::new(None)),
            pc: None,
            viewer_id,
            host_id,
            callback: None,
            connected: Arc::new(AtomicBool::new(false)),
            input_channel: Arc::new(Mutex::new(None)),
            clipboard_channel: Arc::new(Mutex::new(None)),
            ws_disconnected: Arc::new(Notify::new()),
            pic_quality: frame::DEFAULT_JPEG_QUALITY as u32,
            fps: frame::DEFAULT_FPS * 2,
            width: frame::DEFAULT_FRAME_W,
            height: frame::DEFAULT_FRAME_H,
            bitrate: None,
        }
    }

    pub fn set_callback(&mut self, cb: Arc<dyn ViewerCallback>) {
        self.callback = Some(cb);
    }

    /// 设置视频帧率
    pub fn set_fps(&mut self, fps: u32) {
        self.fps = fps;
    }

    pub fn set_pic_quality(&mut self, pic_quality: u32) {
        self.pic_quality = pic_quality;
    }

    /// 设置视频分辨率
    pub fn set_resolution(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// 设置视频比特率（kbps）
    pub fn set_bitrate(&mut self, bitrate: Option<u32>) {
        self.bitrate = bitrate;
    }

    /// Build `CapturerParams` from the current viewer settings.
    /// `max_dimension` is derived from `max(width, height)`.
    fn capturer_params(&self) -> signal::CapturerParams {
        signal::CapturerParams {
            pic_quality: Some(self.pic_quality),
            max_dimension: Some(cmp::max(self.width, self.height)),
            fps: Some(self.fps),
        }
    }

    pub async fn connect(&mut self, server_addr: String, stun_servers: Vec<String>, password: String) -> Result<()> {
        let (ws_write, ws_read) = Self::connect_to_signaling(&server_addr, &self.viewer_id).await?;

        let (ice_tx, ice_rx) = tokio::sync::mpsc::unbounded_channel();
        let (dc_tx, dc_rx) = tokio::sync::mpsc::unbounded_channel();
        let pc = Self::build_peer_connection(&stun_servers, ice_tx, dc_tx, &self.viewer_id).await?;
        self.pc = Some(pc.clone());

        // Spawn handler for incoming data channels
        spawn_dc_handler(
            self.callback.clone(),
            self.connected.clone(),
            self.input_channel.clone(),
            self.clipboard_channel.clone(),
            dc_rx,
        );

        *self.ws_write.lock().await = Some(ws_write);

        // Send connection request to signaling server (with password)
        let req = SignalMessage::ConnectRequest {
            from: self.viewer_id.clone(),
            target_id: self.host_id.clone(),
            pic_quality: self.pic_quality,
            password,
        };
        self.send_signaling_message(&req).await?;
        info!("[{}] ConnectRequest -> {} (quality={})", self.viewer_id, self.host_id, self.pic_quality);

        // Spawn ICE candidate forwarder
        spawn_ice_forwarder(ice_rx, self.host_id.clone(), self.ws_write.clone());

        // Spawn signaling reader (pass captured capturer params for SDP injection)
        let v_params = self.capturer_params();
        self.connected.store(true, Ordering::SeqCst);
        spawn_signaling_reader(
            ws_read,
            self.callback.clone(),
            self.ws_write.clone(),
            self.pc.clone(),
            self.viewer_id.clone(),
            self.host_id.clone(),
            self.connected.clone(),
            self.ws_disconnected.clone(),
            v_params,
        );

        Ok(())
    }


    pub async fn send_mouse_event(&self, x: f64, y: f64, button: u8, pressed: bool) -> Result<()> {
        let ch = self.input_channel.lock().await;
        if let Some(ref dc) = *ch {
            let mut payload = Vec::with_capacity(frame::MOUSE_EVENT_SIZE);
            payload.push(frame::TAG_MOUSE);
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload.push(button);
            payload.push(if pressed { 1 } else { 0 });
            dc.send(BytesMut::from(&payload[..]))
                .await
                .map_err(|e| anyhow::anyhow!("mouse: {}", e))?;
        }
        Ok(())
    }

    pub async fn send_key_event(&self, key_code: u32, pressed: bool) -> Result<()> {
        let ch = self.input_channel.lock().await;
        if let Some(ref dc) = *ch {
            let mut payload = vec![frame::TAG_KEY];
            payload.extend_from_slice(&key_code.to_le_bytes());
            payload.push(if pressed { 1 } else { 0 });
            dc.send(BytesMut::from(&payload[..]))
                .await
                .map_err(|e| anyhow::anyhow!("key: {}", e))?;
        }
        Ok(())
    }

    /// Send a mouse wheel (scroll) event.
    pub async fn send_wheel_event(&self, dx: i32, dy: i32) -> Result<()> {
        let ch = self.input_channel.lock().await;
        if let Some(ref dc) = *ch {
            let mut payload = Vec::with_capacity(frame::WHEEL_EVENT_SIZE);
            payload.push(frame::TAG_WHEEL);
            payload.extend_from_slice(&dx.to_le_bytes());
            payload.extend_from_slice(&dy.to_le_bytes());
            dc.send(BytesMut::from(&payload[..]))
                .await
                .map_err(|e| anyhow::anyhow!("wheel: {}", e))?;
        }
        Ok(())
    }

    /// Send a quality update (quality, max_dimension, fps) to the remote host
    /// during an active session. The host will apply the new settings immediately.
    pub async fn send_quality_update(
        &self,
        quality: u32,
        max_dimension: u32,
        fps: u32,
    ) -> Result<()> {
        let msg = SignalMessage::QualityUpdate {
            from: self.viewer_id.clone(),
            to: self.host_id.clone(),
            quality,
            max_dimension,
            fps,
        };
        self.send_signaling_message(&msg).await
    }

    /// Send raw clipboard data to the remote host.
    pub async fn send_clipboard_data(&self, data: Vec<u8>) -> Result<()> {
        let ch = self.clipboard_channel.lock().await;
        if let Some(ref dc) = *ch {
            dc.send(BytesMut::from(&data[..]))
                .await
                .map_err(|e| anyhow::anyhow!("clipboard: {}", e))?;
        }
        Ok(())
    }

    /// Send file list + pre-serialized chunk data to the remote host for clipboard paste.
    /// `file_list` is the header (including 0x03 prefix and all file metadata).
    /// `chunks` is a flat byte array of pre-packed [0x04][index:u16][off:u64][len:u32][data]* chunks.
    ///
    /// Each chunk is sent as an individual DataChannel message to avoid SCTP
    /// message size limits and allow the host to process them one at a time.
    pub async fn send_files(&self, file_list: Vec<u8>, chunks: Vec<u8>) -> Result<()> {
        let ch = self.clipboard_channel.lock().await;
        if let Some(ref dc) = *ch {
            // Send file list header (already includes 0x03 prefix)
            dc.send(BytesMut::from(&file_list[..]))
                .await
                .map_err(|e| anyhow::anyhow!("file_list: {}", e))?;

            // Split the flat chunks blob into individual chunk messages.
            // Format per chunk: [0x04][file_index:u16le][offset:u64le][len:u32le][data]
            let msg_sz = frame::CLIP_CHUNK_MSG_SIZE;
            let mut offset = 0usize;
            while offset + msg_sz <= chunks.len() {
                if chunks[offset] != frame::CLIP_TAG_FILE_CHUNK {
                    warn!(
                        "[Viewer] Expected chunk marker at offset {}, got 0x{:02X}",
                        offset, chunks[offset]
                    );
                    offset += 1;
                    continue;
                }
                let data_len = u32::from_le_bytes(
                    chunks[offset + 11..offset + msg_sz]
                        .try_into()
                        .unwrap_or([0; 4]),
                ) as usize;
                let total = msg_sz + data_len;
                if offset + total > chunks.len() {
                    warn!("Chunk truncated at offset {}", offset);
                    break;
                }
                dc.send(BytesMut::from(&chunks[offset..offset + total]))
                    .await
                    .map_err(|e| anyhow::anyhow!("file_chunk: {}", e))?;
                offset += total;
            }

            // Send complete marker
            dc.send(BytesMut::from(&[frame::CLIP_TAG_DONE][..]))
                .await
                .map_err(|e| anyhow::anyhow!("file_complete: {}", e))?;
        }
        Ok(())
    }

    pub async fn disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        if let Some(ref pc) = self.pc {
            if let Err(e) = pc.close().await {
                error!("[Viewer] disconnect close error: {}", e);
            }
        }
        // Close the WebSocket writer so the signaling server detects
        // the disconnection and removes this peer from the registry.
        // Without this, the old WS stays open and the server never
        // sends PeerLeft to the host, blocking reconnection.
        let mut ws = self.ws_write.lock().await;
        if let Some(w) = ws.take() {
            drop(w); // explicitly drop to trigger WS close
        }
        self.ws_disconnected.notify_one();
    }

    // ── Private helpers for connect() ─────────────────────────────

    async fn connect_to_signaling(
        server_addr: &str,
        viewer_id: &str,
    ) -> Result<(
        WsSplitSink,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    )> {
        use futures_util::{SinkExt, StreamExt};

        // Load credentials from config
        let cfg = crate::config::load_config();
        let username = cfg.login_username.clone();
        let password = cfg.login_password.clone();

        let ws_url = format!("ws://{}/ws", server_addr);
        info!("[{}] Connecting to {}", viewer_id, ws_url);
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .context("failed to connect to signaling server")?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Phase 1: Login
        let login_msg = SignalMessage::Login {
            username: username.clone(),
            password: password.clone(),
        };
        ws_write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&login_msg)?.into(),
            ))
            .await
            .map_err(|_| anyhow::anyhow!("login send error"))?;
        info!("[{}] Login sent as '{}', waiting for LoginResult", viewer_id, username);

        // Wait for LoginResult
        loop {
            let Some(msg) = ws_read.next().await else {
                anyhow::bail!("WS closed before LoginResult");
            };
            let text = match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Text(t)) => t,
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                    anyhow::bail!("WS closed before LoginResult");
                }
                Err(e) => anyhow::bail!("WS error before LoginResult: {}", e),
                _ => continue,
            };
            let sm: SignalMessage = serde_json::from_str(&text)
                .context("failed to parse LoginResult")?;
            match sm {
                SignalMessage::LoginResult { success, token, reason } => {
                    if success {
                        info!("[{}] Login successful, token received", viewer_id);
                        let _ = token; // token stored for future refresh
                        break;
                    } else {
                        anyhow::bail!("Login failed: {}", reason);
                    }
                }
                SignalMessage::Error { message } => {
                    anyhow::bail!("Server error during login: {}", message);
                }
                _ => {
                    info!("[{}] Ignoring unexpected message during login: {:?}", viewer_id, sm);
                }
            }
        }

        // Phase 2: Register as Viewer
        let register = SignalMessage::Register {
            id: viewer_id.to_string(),
            role: PeerRole::Viewer,
        };
        ws_write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&register)?.into(),
            ))
            .await
            .map_err(|_| anyhow::anyhow!("register send error"))?;
        info!("[{}] Registered as Viewer", viewer_id);
        Ok((ws_write, ws_read))
    }

    async fn build_peer_connection(
        stun: &[String],
        ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
        dc_tx: tokio::sync::mpsc::UnboundedSender<Arc<dyn DataChannel>>,
        peer_id: &str,
    ) -> Result<Arc<dyn PeerConnection>> {
        let handler = Arc::new(SignalingEventHandler::new(
            ice_tx,
            dc_tx,
            peer_id.to_string(),
        ));
        let pc = PeerConnectionBuilder::new()
            .with_configuration(build_configuration(stun))
            .with_handler(handler)
            .with_udp_addrs(discover_udp_addrs())
            .build()
            .await
            .context("build peer connection")?;
        Ok(Arc::from(Box::new(pc) as Box<dyn PeerConnection>))
    }

    async fn send_signaling_message(&self, msg: &SignalMessage) -> Result<()> {
        use futures_util::SinkExt;
        let mut ws = self.ws_write.lock().await;
        if let Some(ref mut w) = *ws {
            w.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(msg)?.into(),
            ))
            .await
            .map_err(|_| anyhow::anyhow!("WS send error"))?;
        }
        Ok(())
    }
}

// ── Spawned task helpers (free functions) ────────────────────────────

fn spawn_dc_handler(
    callback: Option<Arc<dyn ViewerCallback>>,
    connected: Arc<AtomicBool>,
    input_ch: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_ch: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    mut dc_rx: tokio::sync::mpsc::UnboundedReceiver<Arc<dyn DataChannel>>,
) {
    tokio::spawn(async move {
        while let Some(dc) = dc_rx.recv().await {
            let label = dc.label().await.unwrap_or_default();
            info!("[DC] Processing: {}", label);
            match label.as_str() {
                "desktop" => {
                    tokio::spawn(assemble_desktop_frames(
                        callback.clone(),
                        connected.clone(),
                        dc,
                    ));
                }
                "input" => {
                    *input_ch.lock().await = Some(dc);
                }
                "clipboard" => {
                    let dc_read = dc.clone();
                    *clipboard_ch.lock().await = Some(dc);
                    tokio::spawn(viewer_clipboard_reader(
                        dc_read,
                        callback.clone(),
                        connected.clone(),
                    ));
                }
                _ => {}
            }
        }
    });
}

async fn assemble_desktop_frames(
    callback: Option<Arc<dyn ViewerCallback>>,
    connected: Arc<AtomicBool>,
    dc: Arc<dyn DataChannel>,
) {
    let mut assembler = FrameAssembler::new();
    while connected.load(Ordering::Relaxed) {
        match dc.poll().await {
            Some(DataChannelEvent::OnMessage(msg)) => {
                if let Some(merged) = assembler.push(&msg.data) {
                    if merged.len() >= frame::FRAME_HDR_SIZE {
                        let orig_w = u32::from_be_bytes(
                            merged[frame::FRAME_ORIG_W_OFF..frame::FRAME_ORIG_H_OFF]
                                .try_into()
                                .unwrap_or([0; 4]),
                        );
                        let orig_h = u32::from_be_bytes(
                            merged[frame::FRAME_ORIG_H_OFF..frame::FRAME_W_OFF]
                                .try_into()
                                .unwrap_or([0; 4]),
                        );
                        let w = u32::from_be_bytes(
                            merged[frame::FRAME_W_OFF..frame::FRAME_H_OFF]
                                .try_into()
                                .unwrap_or([0; 4]),
                        );
                        let h = u32::from_be_bytes(
                            merged[frame::FRAME_H_OFF..frame::FRAME_HDR_SIZE]
                                .try_into()
                                .unwrap_or([0; 4]),
                        );
                        if let Some(ref c) = callback {
                            c.on_frame(merged[frame::FRAME_HDR_SIZE..].to_vec(), orig_w, orig_h, w, h);
                        }
                    }
                }
            }
            Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => break,
            None => tokio::time::sleep(std::time::Duration::from_millis(frame::VIEWER_FRAME_POLL_MS)).await,
            _ => {}
        }
    }
}

// ── Viewer clipboard reader (receives host→viewer clipboard data) ─────

struct ClipViewerState {
    files: Vec<(std::path::PathBuf, std::fs::File, u64)>,
    dir: std::path::PathBuf,
}

async fn viewer_clipboard_reader(
    dc: Arc<dyn DataChannel>,
    callback: Option<Arc<dyn ViewerCallback>>,
    connected: Arc<AtomicBool>,
) {
    info!("[Viewer] Clipboard reader started");
    let mut state: Option<ClipViewerState> = None;
    while connected.load(Ordering::Relaxed) {
        match dc.poll().await {
            Some(DataChannelEvent::OnMessage(msg)) => {
                if !msg.data.is_empty() {
                    state = handle_viewer_clipboard_msg(&msg.data, state, &callback);
                }
            }
            Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => {
                info!("[Viewer] Clipboard channel closed");
                break;
            }
            None => {
                tokio::time::sleep(std::time::Duration::from_millis(frame::CLIPBOARD_POLL_MS)).await;
            }
            _ => {}
        }
    }
    info!("[Viewer] Clipboard reader ended");
}

fn handle_viewer_clipboard_msg(
    data: &[u8],
    state: Option<ClipViewerState>,
    callback: &Option<Arc<dyn ViewerCallback>>,
) -> Option<ClipViewerState> {
    match data[0] {
        frame::CLIP_TAG_TEXT => {
            // Text clipboard from host
            let text = String::from_utf8_lossy(&data[1..]).to_string();
            if let Some(c) = callback {
                c.on_clipboard_text(text);
            }
            state
        }
        frame::CLIP_TAG_IMAGE => {
            // Image clipboard from host: [width:u32le][height:u32le][rgba_data...]
            if data.len() < 1 + frame::CLIP_IMAGE_HDR_SIZE {
                warn!("[Viewer] Image clipboard msg too short: {} bytes", data.len());
                return state;
            }
            let width = u32::from_le_bytes(data[1..5].try_into().unwrap_or_default());
            let height = u32::from_le_bytes(data[5..9].try_into().unwrap_or_default());
            let rgba = data[9..].to_vec();
            if let Some(c) = callback {
                c.on_clipboard_image(rgba, width, height);
            }
            state
        }
        frame::CLIP_TAG_FILE_LIST => handle_viewer_file_list(&data[1..]),
        frame::CLIP_TAG_FILE_CHUNK => handle_viewer_file_chunk(&data[1..], state),
        frame::CLIP_TAG_DONE => {
            if let Some(s) = state {
                handle_viewer_transfer_complete(s, callback);
            }
            None
        }
        _ => {
            warn!("[Viewer] Unknown clipboard msg type: 0x{:02X}", data[0]);
            state
        }
    }
}

fn handle_viewer_file_list(data: &[u8]) -> Option<ClipViewerState> {
    if data.len() < 2 {
        return None;
    }
    let count = u16::from_le_bytes([data[0], data[1]]) as usize;
    let mut offset = 2usize;
    let mut files: Vec<(std::path::PathBuf, std::fs::File, u64)> = Vec::new();

    let dir = std::env::temp_dir().join("rtcdesk-clipboard");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        error!("[Viewer] Failed to create temp dir {}: {}", dir.display(), e);
    }

    for _ in 0..count {
        if offset + 2 > data.len() {
            break;
        }
        let name_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + name_len > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
        offset += name_len;
        if offset + 8 > data.len() {
            break;
        }
        let size = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap_or_default());
        offset += 8;

        let fname = std::path::PathBuf::from(&name)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file_{}", files.len()));
        let path = dir.join(&fname);

        match std::fs::File::create(&path) {
            Ok(f) => files.push((path, f, size)),
            Err(e) => error!("[Viewer] Cannot create file {}: {}", fname, e),
        }
    }

    if files.is_empty() {
        None
    } else {
        Some(ClipViewerState { files, dir })
    }
}

fn handle_viewer_file_chunk(
    data: &[u8],
    state: Option<ClipViewerState>,
) -> Option<ClipViewerState> {
    let mut s = state?;
    let mut offset = 0usize;
    while offset + 14 <= data.len() {
        let idx = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        let off = u64::from_le_bytes(data[offset + 2..offset + 10].try_into().unwrap_or_default());
        let len = u32::from_le_bytes(
            data[offset + 10..offset + 14]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let header_end = offset + 14;
        if data.len() < header_end + len {
            break;
        }
        let chunk_data = &data[header_end..header_end + len];
        if idx < s.files.len() {
            let file = &mut s.files[idx].1;
            if let Err(e) = file.seek(SeekFrom::Start(off)) {
                error!("[Viewer] Seek error: {}", e);
            } else if let Err(e) = file.write_all(chunk_data) {
                error!("[Viewer] Write error: {}", e);
            }
        }
        offset = header_end + len;
    }
    Some(s)
}

fn handle_viewer_transfer_complete(s: ClipViewerState, callback: &Option<Arc<dyn ViewerCallback>>) {
    let file_count = s.files.len();
    let dir_display = s.dir.display().to_string();
    let uris: Vec<String> = s
        .files
        .iter()
        .map(|(path, _, _)| format!("file://{}", path.display()))
        .collect();
    info!(
        "[Viewer] Host clipboard files received: {} files in {}",
        file_count, dir_display
    );
    // File handles are dropped automatically when s goes out of scope.
    drop(s);
    if let Some(c) = callback {
        c.on_clipboard_file_uris(uris.join("\n"));
    }
}

fn spawn_ice_forwarder(
    mut ice_rx: tokio::sync::mpsc::UnboundedReceiver<SignalMessage>,
    host_id: String,
    ws_write: Arc<Mutex<Option<WsSplitSink>>>,
) {
    use futures_util::SinkExt;
    tokio::spawn(async move {
        while let Some(mut msg) = ice_rx.recv().await {
            if let SignalMessage::IceCandidate { to, .. } = &mut msg {
                *to = host_id.clone();
            }
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("[Viewer] ICE candidate serialize error: {}", e);
                    continue;
                }
            };
            let mut ws = ws_write.lock().await;
            if let Some(ref mut w) = *ws {
                if let Err(e) = w
                    .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                    .await
                {
                    error!("[Viewer] ICE candidate WS send error: {}", e);
                }
            }
        }
    });
}

fn spawn_signaling_reader(
    mut ws_read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    callback: Option<Arc<dyn ViewerCallback>>,
    ws_write: Arc<Mutex<Option<WsSplitSink>>>,
    pc: Option<Arc<dyn PeerConnection>>,
    viewer_id: String,
    host_id: String,
    connected: Arc<AtomicBool>,
    ws_disconnected: Arc<Notify>,
    v_params: signal::CapturerParams,
) {
    use futures_util::StreamExt;
    tokio::spawn(async move {
        info!("[{}] Signaling reader started", viewer_id);
        while let Some(msg) = ws_read.next().await {
            if dispatch_viewer_signal(
                msg,
                &callback,
                &ws_write,
                &pc,
                &viewer_id,
                &host_id,
                &connected,
                &ws_disconnected,
                &v_params,
            )
            .await
            {
                return;
            }
        }
        connected.store(false, Ordering::SeqCst);
        ws_disconnected.notify_one();
    });
}

async fn dispatch_viewer_signal(
    msg: Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    callback: &Option<Arc<dyn ViewerCallback>>,
    ws_write: &Arc<Mutex<Option<WsSplitSink>>>,
    pc: &Option<Arc<dyn PeerConnection>>,
    viewer_id: &str,
    host_id: &str,
    connected: &Arc<AtomicBool>,
    ws_disconnected: &Arc<Notify>,
    v_params: &signal::CapturerParams,
) -> bool {
    use tokio_tungstenite::tungstenite::Message as TM;
    match msg {
        Ok(TM::Text(text)) => {
            let sm: SignalMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    warn!("Bad msg: {}", e);
                    return false;
                }
            };
            info!("[{}] Recv: {:?}", viewer_id, sm);
            handle_offer(
                sm,
                callback,
                ws_write,
                pc,
                viewer_id,
                host_id,
                connected,
                ws_disconnected,
                v_params,
            )
            .await
        }
        Ok(TM::Close(_)) | Err(_) => {
            connected.store(false, Ordering::SeqCst);
            ws_disconnected.notify_one();
            if let Some(c) = callback {
                c.on_disconnected();
            }
            true
        }
        _ => false,
    }
}

async fn handle_offer(
    sm: SignalMessage,
    callback: &Option<Arc<dyn ViewerCallback>>,
    ws_write: &Arc<Mutex<Option<WsSplitSink>>>,
    pc: &Option<Arc<dyn PeerConnection>>,
    viewer_id: &str,
    host_id: &str,
    connected: &Arc<AtomicBool>,
    ws_disconnected: &Arc<Notify>,
    v_params: &signal::CapturerParams,
) -> bool {
    match sm {
        SignalMessage::Registered { .. } => {
            if let Some(c) = callback {
                c.on_connection_state("registered".into());
            }
        }
        SignalMessage::Offer { sdp, .. } => {
            info!("[{}] Offer received", viewer_id);
            process_offer(sdp, callback, ws_write, pc, viewer_id, host_id, v_params).await;
        }
        SignalMessage::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            if let Some(pc) = pc {
                if let Err(e) = pc
                    .add_ice_candidate(webrtc::peer_connection::RTCIceCandidateInit {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        username_fragment: None,
                        url: None,
                    })
                    .await
                {
                    error!("[Viewer] add_ice_candidate error: {}", e);
                }
            }
        }
        SignalMessage::PeerLeft { id } if id == *host_id => {
            connected.store(false, Ordering::SeqCst);
            ws_disconnected.notify_one();
            if let Some(c) = callback {
                c.on_disconnected();
            }
            return true;
        }
        SignalMessage::Error { message } => {
            if let Some(c) = callback {
                c.on_error(message);
            }
        }
        SignalMessage::AuthResult { accepted, reason, .. } => {
            if accepted {
                info!("[{}] AuthResult: accepted", viewer_id);
                if let Some(c) = callback {
                    c.on_connection_state("auth_accepted".into());
                }
            } else {
                let msg = if reason.is_empty() {
                    "认证失败".to_string()
                } else {
                    format!("认证失败: {}", reason)
                };
                warn!("[{}] AuthResult: rejected – {}", viewer_id, msg);
                if let Some(c) = callback {
                    c.on_error(msg);
                }
            }
        }
        _ => {}
    }
    false
}

async fn process_offer(
    sdp: String,
    callback: &Option<Arc<dyn ViewerCallback>>,
    ws_write: &Arc<Mutex<Option<WsSplitSink>>>,
    pc: &Option<Arc<dyn PeerConnection>>,
    viewer_id: &str,
    host_id: &str,
    v_params: &signal::CapturerParams,
) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TM;
    let Some(pc) = pc else { return };
    let desc = match RTCSessionDescription::offer(sdp) {
        Ok(d) => d,
        Err(e) => {
            error!("offer: {}", e);
            return;
        }
    };
    if let Err(e) = pc.set_remote_description(desc).await {
        error!("set_remote: {}", e);
        return;
    }
    match pc.create_answer(None).await {
        Ok(answer) => {
            if let Err(e) = pc.set_local_description(answer.clone()).await {
                error!("[Viewer] set_local_description failed: {}", e);
                return;
            }
            let sdp_with_params = format!(
                "{}\n{}",
                answer.sdp,
                signal::encode_capturer_params_to_sdp(v_params),
            );
            info!(
                "[{}] Injecting capturer params into Answer SDP: quality={:?} max_dim={:?} fps={:?}",
                viewer_id, v_params.pic_quality, v_params.max_dimension, v_params.fps
            );
            let am = SignalMessage::Answer {
                from: viewer_id.to_string(),
                to: host_id.to_string(),
                sdp: sdp_with_params,
            };
            let json = match serde_json::to_string(&am) {
                Ok(j) => j,
                Err(e) => {
                    error!("[Viewer] Answer serialize error: {}", e);
                    return;
                }
            };
            let mut ws = ws_write.lock().await;
            if let Some(ref mut w) = *ws {
                if let Err(e) = w.send(TM::Text(json.into())).await {
                    error!("[Viewer] Answer WS send error: {}", e);
                    return;
                }
            }
            info!("[{}] Answer sent", viewer_id);
            if let Some(c) = callback {
                c.on_connected();
            }
        }
        Err(e) => error!("create_answer: {}", e),
    }
}

#[derive(Clone)]
struct SignalingEventHandler {
    ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    dc_tx: tokio::sync::mpsc::UnboundedSender<Arc<dyn DataChannel>>,
    peer_id: String,
}

impl SignalingEventHandler {
    fn new(
        ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
        dc_tx: tokio::sync::mpsc::UnboundedSender<Arc<dyn DataChannel>>,
        peer_id: String,
    ) -> Self {
        Self {
            ice_tx,
            dc_tx,
            peer_id,
        }
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SignalingEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(init) = event.candidate.to_json() {
            if init.candidate.is_empty() {
                return;
            }
            info!("[{}] ICE candidate generated", self.peer_id);
            let msg = SignalMessage::IceCandidate {
                from: self.peer_id.clone(),
                to: String::new(),
                candidate: init.candidate,
                sdp_mid: init.sdp_mid,
                sdp_mline_index: init.sdp_mline_index,
            };
            if let Err(e) = self.ice_tx.send(msg) {
                error!("[{}] ICE candidate send error: {}", self.peer_id, e);
            }
        }
    }
    async fn on_ice_connection_state_change(&self, s: RTCIceConnectionState) {
        info!("[{}] ICE: {:?}", self.peer_id, s);
    }
    async fn on_connection_state_change(&self, s: RTCPeerConnectionState) {
        info!("[{}] Conn: {:?}", self.peer_id, s);
    }
    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let label = dc.label().await.unwrap_or_default();
        info!("[{}] Remote DC '{}'", self.peer_id, label);
        if let Err(e) = self.dc_tx.send(dc) {
            error!("[{}] dc_tx send error: {}", self.peer_id, e);
        }
    }
}

fn build_configuration(stun: &[String]) -> RTCConfiguration {
    let servers: Vec<RTCIceServer> = if stun.is_empty() {
        vec![RTCIceServer {
            urls: vec![frame::DEFAULT_STUN.into()],
            ..Default::default()
        }]
    } else {
        stun.iter()
            .map(|u| RTCIceServer {
                urls: vec![u.clone()],
                ..Default::default()
            })
            .collect()
    };
    RTCConfigurationBuilder::new()
        .with_ice_servers(servers)
        .build()
}
