//! WebRTC viewer client using webrtc-rs PeerConnection.
//!
//! Handles WebSocket signaling AND the WebRTC peer connection entirely in Rust.
//! Video frames are decoded in Rust and rendered via <img> data URLs —
//! no JavaScript WebRTC bridge is needed.
//!
//! DataChannels for input and clipboard are also created and managed in Rust.

use anyhow::{Context, Result};
use bytes::BytesMut;
use log::{error, info, warn};
use shared::signal::{PeerRole, SignalMessage};
use shared::{frame, signal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

use rtc::interceptor::Registry;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264};
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind};
use rtc::rtp_transceiver::RTCRtpTransceiverDirection;
use rtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceConnectionState, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    RTCSessionDescription,
};
use webrtc::runtime::{default_runtime, Runtime};

use crate::decoder::{FrameDecoder, RtpDepacketizer};

// ─── Viewer callback ─────────────────────────────────────────────────────

pub trait ViewerCallback: Send + Sync {
    fn on_connected(&self);
    fn on_disconnected(&self);
    fn on_connection_state(&self, state: String);
    fn on_error(&self, message: String);
    fn on_video_frame(&self, bgra: &[u8], width: u32, height: u32);
    fn on_clipboard_text(&self, text: String);
    fn on_clipboard_file_uris(&self, uris: String);
    fn on_clipboard_image(&self, rgba: Vec<u8>, width: u32, height: u32);
    fn on_data_channel_open(&self, label: String);
}

// ─── WebSocket types ─────────────────────────────────────────────────────

type WsSplitSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

type WsSplitStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

// ─── RtcViewer ───────────────────────────────────────────────────────────

/// Manages the WebSocket signaling connection AND the webrtc-rs PeerConnection.
///
/// Video is received via the video track, decoded in Rust, and
/// rendered as JPEG frames.  Input and clipboard use DataChannels
/// that are created and polled entirely in Rust.
pub struct RtcViewer {
    ws_write: Arc<Mutex<Option<WsSplitSink>>>,
    viewer_id: String,
    host_id: String,
    callback: Arc<dyn ViewerCallback>,
    connected: Arc<AtomicBool>,
    pub ws_disconnected: Arc<Notify>,
    pic_quality: u32,
    fps: u32,
    width: u32,
    height: u32,
    #[allow(dead_code)]
    bitrate: Option<u32>,
    /// The webrtc-rs PeerConnection (created when Offer arrives).
    pc: Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
    /// Input DataChannel (created by host, we receive it).
    input_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    /// Clipboard DataChannel (created by host, we receive it).
    clipboard_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    /// Running flag for the peer connection.
    running: Arc<AtomicBool>,
    /// Runtime for spawning tasks inside the event handler.
    runtime: Arc<dyn Runtime>,
    /// Sender end of the H264 frame channel (decode thread reads from the receiver).
    /// Created at construction so the decoder is initialized before any
    /// WebRTC track arrives — this eliminates decoder startup latency.
    /// Bounded channel (cap=3) provides backpressure; try_send() drops
    /// frames when full and requests a keyframe for clean restart.
    frame_sender: std::sync::mpsc::SyncSender<Vec<u8>>,
    /// Whether the decoder in the decode thread is fully initialized.
    decoder_ready: Arc<AtomicBool>,
    /// Shared need-keyframe flag between decode thread and on_track loop.
    /// The decode thread sets it when it needs a keyframe; the on_track
    /// loop reads it and sends PLI.
    need_keyframe: Arc<AtomicBool>,
}

impl RtcViewer {
    pub fn new(viewer_id: String, host_id: String, cb: Arc<dyn ViewerCallback>) -> Self {
        let runtime = default_runtime().expect("no async runtime available");

        // Pre-initialize the decoder by spawning the decode thread now.
        // This way, when a WebRTC video track arrives (on_track),
        // the decoder is already ready and no startup latency is incurred.
        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(3);
        let decoder_ready = Arc::new(AtomicBool::new(false));
        let need_keyframe = Arc::new(AtomicBool::new(true)); // start needing keyframe
        let running = Arc::new(AtomicBool::new(false));

        let decoder_ready_flag = decoder_ready.clone();
        let need_kf_flag = need_keyframe.clone();
        let cb_ref = cb.clone();
        let vid = viewer_id.clone();

        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                let mut decoder = FrameDecoder::new();
                info!("[{}] Decode thread started, decoder initialized", vid);

                // Signal that the decoder is ready — host should wait for
                // this before streaming to avoid sending frames to an
                // unready decoder (SPS/PPS get lost or arrive too early).
                decoder_ready_flag.store(true, Ordering::Relaxed);

                // Decode ALL frames sequentially (preserves H264 reference
                // chain). Do NOT drain-to-latest — that breaks reference
                // frames and causes "reference picture missing" errors.
                // The FrameSlot always holds the latest decoded frame,
                // so the UI naturally displays the most recent one.
                while let Ok(h264_data) = frame_rx.recv() {
                    let (result, need_kf) = decoder.decode_frame(&h264_data);
                    if need_kf {
                        need_kf_flag.store(true, Ordering::Relaxed);
                    }
                    if let Some(decoded) = result {
                        need_kf_flag.store(false, Ordering::Relaxed);
                        cb_ref.on_video_frame(&decoded.bgra_data, decoded.width, decoded.height);
                    }
                }

                info!("[{}] Decode thread ended", vid);
            })
            .expect("failed to spawn decode thread");

        Self {
            ws_write: Arc::new(Mutex::new(None)),
            viewer_id,
            host_id,
            callback: cb,
            connected: Arc::new(AtomicBool::new(false)),
            ws_disconnected: Arc::new(Notify::new()),
            pic_quality: frame::DEFAULT_PIC_QUALITY,
            fps: frame::DEFAULT_FPS * 2,
            width: frame::DEFAULT_FRAME_W,
            height: frame::DEFAULT_FRAME_H,
            bitrate: None,
            pc: Arc::new(Mutex::new(None)),
            input_dc: Arc::new(Mutex::new(None)),
            clipboard_dc: Arc::new(Mutex::new(None)),
            running,
            runtime,
            frame_sender: frame_tx,
            decoder_ready,
            need_keyframe,
        }
    }

    pub fn set_fps(&mut self, fps: u32) {
        self.fps = fps;
    }

    pub fn set_pic_quality(&mut self, pic_quality: u32) {
        self.pic_quality = pic_quality;
    }

    pub fn set_resolution(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    #[allow(dead_code)]
    pub fn set_bitrate(&mut self, bitrate: Option<u32>) {
        self.bitrate = bitrate;
    }

    pub fn capturer_params(&self) -> signal::CapturerParams {
        signal::CapturerParams {
            pic_quality: Some(self.pic_quality),
            max_dimension: Some(std::cmp::max(self.width, self.height)),
            fps: Some(self.fps),
        }
    }

    /// Connect to the signaling server and send a ConnectRequest.
    pub async fn connect(
        &mut self,
        server_addr: String,
        stun_servers: Vec<String>,
        password: String,
    ) -> Result<()> {
        let (ws_write, ws_read) = Self::connect_to_signaling(&server_addr, &self.viewer_id).await?;
        *self.ws_write.lock().await = Some(ws_write);

        let req = SignalMessage::ConnectRequest {
            from: self.viewer_id.clone(),
            target_id: self.host_id.clone(),
            pic_quality: self.pic_quality,
            password,
        };
        self.send_signaling_message(&req).await?;
        info!(
            "[{}] ConnectRequest -> {} (quality={})",
            self.viewer_id, self.host_id, self.pic_quality
        );

        self.connected.store(true, Ordering::SeqCst);

        let v_params = self.capturer_params();
        let stun = stun_servers;
        let callback = self.callback.clone();
        let pc_mutex = self.pc.clone();
        let input_dc = self.input_dc.clone();
        let clipboard_dc = self.clipboard_dc.clone();
        let running = self.running.clone();
        let ws_write = self.ws_write.clone();
        let viewer_id = self.viewer_id.clone();
        let host_id = self.host_id.clone();
        let connected = self.connected.clone();
        let ws_disconnected = self.ws_disconnected.clone();
        let runtime = self.runtime.clone();
        let frame_sender = self.frame_sender.clone();
        let decoder_ready = self.decoder_ready.clone();
        let need_keyframe = self.need_keyframe.clone();

        spawn_signaling_reader(
            ws_read,
            callback,
            ws_write,
            viewer_id,
            host_id,
            connected,
            ws_disconnected,
            v_params,
            stun,
            pc_mutex,
            input_dc,
            clipboard_dc,
            running,
            runtime,
            frame_sender,
            decoder_ready,
            need_keyframe,
        );

        Ok(())
    }

    /// Send mouse event via the input DataChannel.
    pub async fn send_mouse_event(&self, x: f64, y: f64, button: u8, pressed: bool) -> Result<()> {
        let dc = self.input_dc.lock().await;
        if let Some(ref dc) = *dc {
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

    /// Send keyboard event via the input DataChannel.
    pub async fn send_key_event(&self, key_code: u32, pressed: bool) -> Result<()> {
        let dc = self.input_dc.lock().await;
        if let Some(ref dc) = *dc {
            let mut payload = vec![frame::TAG_KEY];
            payload.extend_from_slice(&key_code.to_le_bytes());
            payload.push(if pressed { 1 } else { 0 });
            dc.send(BytesMut::from(&payload[..]))
                .await
                .map_err(|e| anyhow::anyhow!("key: {}", e))?;
        }
        Ok(())
    }

    /// Send mouse wheel (scroll) event via the input DataChannel.
    pub async fn send_wheel_event(&self, dx: i32, dy: i32) -> Result<()> {
        let dc = self.input_dc.lock().await;
        if let Some(ref dc) = *dc {
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
        bitrate_kbps: u32,
    ) -> Result<()> {
        let msg = SignalMessage::QualityUpdate {
            from: self.viewer_id.clone(),
            to: self.host_id.clone(),
            quality,
            max_dimension,
            fps,
            bitrate_kbps,
        };
        self.send_signaling_message(&msg).await
    }

    /// Send clipboard data via the clipboard DataChannel.
    pub async fn send_clipboard(&self, data: &[u8]) -> Result<()> {
        let dc = self.clipboard_dc.lock().await;
        if let Some(ref dc) = *dc {
            let _ = dc.send(BytesMut::from(data)).await;
        }
        Ok(())
    }

    pub async fn disconnect(&self) {
        self.running.store(false, Ordering::SeqCst);
        // Close PeerConnection
        let pc = self.pc.lock().await;
        if let Some(ref pc) = *pc {
            let _ = pc.close().await;
        }
        drop(pc);
        // Close WebSocket
        let mut ws = self.ws_write.lock().await;
        if let Some(w) = ws.take() {
            drop(w);
        }
        self.ws_disconnected.notify_one();
    }

    // ── Private helpers ────────────────────────────────────────────

    async fn connect_to_signaling(
        server_addr: &str,
        viewer_id: &str,
    ) -> Result<(WsSplitSink, WsSplitStream)> {
        use futures_util::{SinkExt, StreamExt};

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
        info!(
            "[{}] Login sent as '{}', waiting for LoginResult",
            viewer_id, username
        );

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
            let sm: SignalMessage =
                serde_json::from_str(&text).context("failed to parse LoginResult")?;
            match sm {
                SignalMessage::LoginResult {
                    success,
                    token,
                    reason,
                } => {
                    if success {
                        info!("[{}] Login successful, token received", viewer_id);
                        let _ = token;
                        break;
                    } else {
                        anyhow::bail!("Login failed: {}", reason);
                    }
                }
                SignalMessage::Error { message } => {
                    anyhow::bail!("Server error during login: {}", message);
                }
                _ => {
                    info!(
                        "[{}] Ignoring unexpected message during login: {:?}",
                        viewer_id, sm
                    );
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

// ─── Build client PeerConnection ─────────────────────────────────────────

/// Build a webrtc-rs PeerConnection configured as a viewer (recvonly video).
async fn build_viewer_peer_connection(
    stun_servers: &[String],
    handler: Arc<dyn PeerConnectionEventHandler>,
    runtime: Arc<dyn Runtime>,
) -> Result<(Arc<dyn PeerConnection>, MediaEngine)> {
    let mut media_engine = MediaEngine::default();

    // Register H264 codec — must match the host's codec config
    let video_codec = RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: 102,
        ..Default::default()
    };
    media_engine.register_codec(video_codec, RtpCodecKind::Video)?;

    let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;

    let ice_servers: Vec<RTCIceServer> = if stun_servers.is_empty() {
        vec![RTCIceServer {
            urls: vec![frame::DEFAULT_STUN.into()],
            ..Default::default()
        }]
    } else {
        stun_servers
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .collect()
    };

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(ice_servers)
        .build();

    let local_ip = match local_ip_address::local_ip() {
        Ok(ip) => format!("{}:0", ip),
        Err(_) => "0.0.0.0:0".to_string(),
    };

    let pc = PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_media_engine(media_engine.clone())
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_runtime(runtime)
        .with_udp_addrs(vec![local_ip])
        .build()
        .await
        .context("failed to build viewer peer connection")?;

    // Add recvonly video transceiver
    pc.add_transceiver_from_kind(
        RtpCodecKind::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            ..Default::default()
        }),
    )
    .await?;

    let pc: Arc<dyn PeerConnection> = Arc::from(Box::new(pc) as Box<dyn PeerConnection>);
    Ok((pc, media_engine))
}

// ─── Signaling reader ───────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn spawn_signaling_reader(
    mut ws_read: WsSplitStream,
    callback: Arc<dyn ViewerCallback>,
    ws_write: Arc<Mutex<Option<WsSplitSink>>>,
    viewer_id: String,
    host_id: String,
    connected: Arc<AtomicBool>,
    ws_disconnected: Arc<Notify>,
    v_params: signal::CapturerParams,
    stun_servers: Vec<String>,
    pc_mutex: Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
    input_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    running: Arc<AtomicBool>,
    runtime: Arc<dyn Runtime>,
    frame_sender: std::sync::mpsc::SyncSender<Vec<u8>>,
    decoder_ready: Arc<AtomicBool>,
    need_keyframe: Arc<AtomicBool>,
) {
    use futures_util::StreamExt;
    tokio::spawn(async move {
        info!("[{}] Signaling reader started", viewer_id);
        while let Some(msg) = ws_read.next().await {
            if handle_viewer_signal(
                msg,
                &callback,
                &ws_write,
                &viewer_id,
                &host_id,
                &connected,
                &ws_disconnected,
                &v_params,
                &stun_servers,
                &pc_mutex,
                &input_dc,
                &clipboard_dc,
                &running,
                &runtime,
                &frame_sender,
                &decoder_ready,
                &need_keyframe,
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

#[allow(clippy::too_many_arguments)]
async fn handle_viewer_signal(
    msg: Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    callback: &Arc<dyn ViewerCallback>,
    ws_write: &Arc<Mutex<Option<WsSplitSink>>>,
    viewer_id: &str,
    host_id: &str,
    connected: &Arc<AtomicBool>,
    ws_disconnected: &Arc<Notify>,
    v_params: &signal::CapturerParams,
    stun_servers: &[String],
    pc_mutex: &Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
    input_dc: &Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_dc: &Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    running: &Arc<AtomicBool>,
    runtime: &Arc<dyn Runtime>,
    frame_sender: &std::sync::mpsc::SyncSender<Vec<u8>>,
    decoder_ready: &Arc<AtomicBool>,
    need_keyframe: &Arc<AtomicBool>,
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
            process_signal_message(
                sm,
                callback,
                ws_write,
                viewer_id,
                host_id,
                connected,
                ws_disconnected,
                v_params,
                stun_servers,
                pc_mutex,
                input_dc,
                clipboard_dc,
                running,
                runtime,
                frame_sender,
                decoder_ready,
                need_keyframe,
            )
            .await
        }
        Ok(TM::Close(_)) | Err(_) => {
            connected.store(false, Ordering::SeqCst);
            ws_disconnected.notify_one();
            callback.on_disconnected();
            true
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_signal_message(
    sm: SignalMessage,
    callback: &Arc<dyn ViewerCallback>,
    ws_write: &Arc<Mutex<Option<WsSplitSink>>>,
    viewer_id: &str,
    host_id: &str,
    _connected: &Arc<AtomicBool>,
    _ws_disconnected: &Arc<Notify>,
    v_params: &signal::CapturerParams,
    stun_servers: &[String],
    pc_mutex: &Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
    input_dc_arc: &Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_dc_arc: &Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    running: &Arc<AtomicBool>,
    runtime: &Arc<dyn Runtime>,
    frame_sender: &std::sync::mpsc::SyncSender<Vec<u8>>,
    decoder_ready: &Arc<AtomicBool>,
    need_keyframe: &Arc<AtomicBool>,
) -> bool {
    match sm {
        SignalMessage::Registered { .. } => {
            callback.on_connection_state("registered".into());
        }
        SignalMessage::AuthResult {
            accepted, reason, ..
        } => {
            if accepted {
                info!("[{}] AuthResult: accepted", viewer_id);
                callback.on_connection_state("auth_accepted".into());
            } else {
                let msg = if reason.is_empty() {
                    "认证失败".to_string()
                } else {
                    format!("认证失败: {}", reason)
                };
                warn!("[{}] AuthResult: rejected – {}", viewer_id, msg);
                callback.on_error(msg);
            }
        }

        // ── Offer SDP received — create PeerConnection and answer ────
        SignalMessage::Offer { sdp, .. } => {
            info!("[{}] Offer received ({} bytes)", viewer_id, sdp.len());

            // Build viewer-side event handler
            let cb = callback.clone();
            let running_clone = running.clone();
            let runtime_clone = runtime.clone();
            let input_dc_clone = input_dc_arc.clone();
            let clipboard_dc_clone = clipboard_dc_arc.clone();

            let handler = Arc::new(ViewerEventHandler {
                viewer_id: viewer_id.to_string(),
                callback: cb,
                running: running_clone,
                runtime: runtime_clone,
                input_dc: input_dc_clone,
                clipboard_dc: clipboard_dc_clone,
                frame_sender: frame_sender.clone(),
                decoder_ready: decoder_ready.clone(),
                need_keyframe: need_keyframe.clone(),
            });

            // Build PeerConnection
            match build_viewer_peer_connection(stun_servers, handler, runtime.clone()).await {
                Ok((pc, _media_engine)) => {
                    // Set running = true BEFORE set_remote_description.
                    // on_track() fires during set_remote_description and spawns
                    // poll tasks that check running — if it's still false, they
                    // exit immediately and never receive any RTP packets.
                    running.store(true, Ordering::SeqCst);

                    // Set remote description (the offer)
                    let offer = match RTCSessionDescription::offer(sdp) {
                        Ok(o) => o,
                        Err(e) => {
                            error!("[{}] Failed to create offer desc: {}", viewer_id, e);
                            running.store(false, Ordering::SeqCst);
                            return false;
                        }
                    };

                    if let Err(e) = pc.set_remote_description(offer).await {
                        error!("[{}] set_remote_description error: {}", viewer_id, e);
                        running.store(false, Ordering::SeqCst);
                        return false;
                    }

                    // Create answer
                    let answer = match pc.create_answer(None).await {
                        Ok(a) => a,
                        Err(e) => {
                            error!("[{}] create_answer error: {}", viewer_id, e);
                            running.store(false, Ordering::SeqCst);
                            return false;
                        }
                    };

                    if let Err(e) = pc.set_local_description(answer.clone()).await {
                        error!("[{}] set_local_description error: {}", viewer_id, e);
                        running.store(false, Ordering::SeqCst);
                        return false;
                    }

                    // Send answer via signaling
                    let sdp_with_params = format!(
                        "{}\n{}",
                        answer.sdp,
                        signal::encode_capturer_params_to_sdp(v_params),
                    );
                    let msg = SignalMessage::Answer {
                        from: viewer_id.to_string(),
                        to: host_id.to_string(),
                        sdp: sdp_with_params,
                    };
                    {
                        use futures_util::SinkExt;
                        let mut ws = ws_write.lock().await;
                        if let Some(ref mut w) = *ws {
                            let _ = w
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&msg).unwrap().into(),
                                ))
                                .await;
                        }
                    }
                    info!("[{}] Answer SDP sent via signaling", viewer_id);

                    // Store the PC
                    *pc_mutex.lock().await = Some(pc);
                }
                Err(e) => {
                    error!(
                        "[{}] Failed to build viewer peer connection: {}",
                        viewer_id, e
                    );
                }
            }
        }

        // ── ICE candidate from host ──
        SignalMessage::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            let pc = pc_mutex.lock().await;
            if let Some(ref pc) = *pc {
                let candidate_init = webrtc::peer_connection::RTCIceCandidateInit {
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                    username_fragment: None,
                    url: None,
                };
                if let Err(e) = pc.add_ice_candidate(candidate_init).await {
                    warn!("[{}] add_ice_candidate error: {}", viewer_id, e);
                }
            }
        }

        SignalMessage::PeerLeft { id } if id == *host_id => {
            running.store(false, Ordering::SeqCst);
            callback.on_disconnected();
            return true;
        }

        SignalMessage::Error { message } => {
            callback.on_error(message);
        }

        _ => {}
    }
    false
}

// ─── Viewer event handler ────────────────────────────────────────────────

struct ViewerEventHandler {
    viewer_id: String,
    callback: Arc<dyn ViewerCallback>,
    running: Arc<AtomicBool>,
    runtime: Arc<dyn Runtime>,
    input_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    clipboard_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    /// Sender to feed H264 frames into the pre-initialized decode thread.
    frame_sender: std::sync::mpsc::SyncSender<Vec<u8>>,
    /// Whether the decoder is ready (pre-initialized at startup).
    decoder_ready: Arc<AtomicBool>,
    /// Shared need-keyframe flag (decode thread → on_track PLI loop).
    need_keyframe: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for ViewerEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        // Viewer doesn't need to forward ICE candidates via callback —
        // they are handled by the signaling reader directly.
        // But we could log them.
        if let Ok(init) = event.candidate.to_json() {
            if !init.candidate.is_empty() {
                info!(
                    "[{}] ICE candidate generated: {}",
                    self.viewer_id, init.candidate
                );
                // TODO: Forward via signaling. For now, ICE candidates are
                // gathered during set_local_description and included in the answer SDP.
            }
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("[{}] Connection state: {:?}", self.viewer_id, state);
        match state {
            RTCPeerConnectionState::Connected => {
                self.running.store(true, Ordering::SeqCst);
                self.callback.on_connected();

                // ── Send VIEWER_READY to host ──────────────────────
                // In this version of webrtc-rs, on_track fires only
                // when the FIRST RTP packet arrives (not during
                // set_remote_description).  The host waits for
                // VIEWER_READY before streaming, so we MUST send
                // it here in on_connection_state_change(Connected)
                // to avoid a deadlock:
                //   host: wait for VIEWER_READY → never streams
                //   client: wait for on_track (first RTP) → never fires
                {
                    let dc = self.input_dc.clone();
                    let ready = self.decoder_ready.clone();
                    let vid = self.viewer_id.clone();
                    self.runtime.spawn(Box::pin(async move {
                        // Wait for decoder to initialize
                        // (should be instant — started at construction time)
                        while !ready.load(Ordering::Relaxed) {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                        // Wait for input DataChannel to open
                        let dc_ref = loop {
                            let guard = dc.lock().await;
                            if let Some(ref ch) = *guard {
                                break ch.clone();
                            }
                            drop(guard);
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        };
                        // Send the ready signal
                        let payload = BytesMut::from(&[frame::TAG_VIEWER_READY][..]);
                        match dc_ref.send(payload).await {
                            Ok(()) => info!("[{}] Sent VIEWER_READY to host", vid),
                            Err(e) => warn!("[{}] Failed to send VIEWER_READY: {}", vid, e),
                        }
                    }));
                }
            }
            RTCPeerConnectionState::Disconnected
            | RTCPeerConnectionState::Failed
            | RTCPeerConnectionState::Closed => {
                self.running.store(false, Ordering::SeqCst);
                self.callback.on_disconnected();
            }
            _ => {}
        }
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        info!("[{}] ICE connection state: {:?}", self.viewer_id, state);
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let label = channel.label().await.unwrap_or_default();
        info!("[{}] DataChannel received: {}", self.viewer_id, label);

        if label == "input" {
            let input_ch = channel.clone();
            *self.input_dc.lock().await = Some(channel);
            self.callback.on_data_channel_open("input".into());

            // Drain internal events (OnBufferedAmountLow, etc.)
            // to prevent the event buffer from filling up.
            let running = self.running.clone();
            self.runtime.spawn(Box::pin(async move {
                while running.load(Ordering::Relaxed) {
                    match input_ch.poll().await {
                        Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => break,
                        None => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                frame::CLIPBOARD_POLL_MS,
                            ))
                            .await;
                        }
                        _ => {} // drain everything else
                    }
                }
            }));
        } else if label == "clipboard" {
            let cb = self.callback.clone();
            let peer_id = self.viewer_id.clone();
            let clip_dc = channel.clone();
            *self.clipboard_dc.lock().await = Some(channel);
            self.callback.on_data_channel_open("clipboard".into());

            // Spawn clipboard reader
            let running = self.running.clone();
            self.runtime.spawn(Box::pin(async move {
                info!("[{}] Clipboard reader started", peer_id);
                while running.load(Ordering::Relaxed) {
                    match clip_dc.poll().await {
                        Some(DataChannelEvent::OnMessage(msg)) => {
                            if !msg.data.is_empty() {
                                handle_clipboard_msg(&msg.data, &cb, &peer_id);
                            }
                        }
                        Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => {
                            info!("[{}] Clipboard channel closed", peer_id);
                            break;
                        }
                        None => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                frame::CLIPBOARD_POLL_MS,
                            ))
                            .await;
                        }
                        _ => {}
                    }
                }
            }));
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let ssrcs = track.ssrcs().await;
        let media_ssrc = ssrcs.first().copied().unwrap_or(0);
        let kind = track.kind().await;
        info!(
            "[{}] Track received: kind={:?} ssrc={}",
            self.viewer_id, kind, media_ssrc
        );

        if kind != RtpCodecKind::Video {
            return;
        }

        let running = self.running.clone();
        let track_clone = track.clone();
        let viewer_id = self.viewer_id.clone();
        let frame_sender = self.frame_sender.clone();
        let need_keyframe = self.need_keyframe.clone();

        // Decoded frames are delivered directly to the callback by the
        // decode thread (via the shared decode_callback mutex).

        // ── RTP packet loop (fast, non-blocking) ─────────────────
        self.runtime.spawn(Box::pin(async move {
            let mut depacketizer = RtpDepacketizer::new();
            info!("[{}] Video track packet collector started", viewer_id);

            while running.load(Ordering::Relaxed) {
                // Send PLI if the decoder needs a keyframe
                if need_keyframe.load(Ordering::Relaxed) {
                    let pli = PictureLossIndication {
                        sender_ssrc: 0, // viewer doesn't have an SSRC
                        media_ssrc,
                    };
                    let _ = track_clone.write_rtcp(vec![Box::new(pli)]).await;
                }

                match track_clone.poll().await {
                    Some(TrackRemoteEvent::OnRtpPacket(packet)) => {
                        let marker = packet.header.marker;
                        let payload = &packet.payload;

                        // Fast depacketization only — no GPU/CPU decode here
                        let (frame_data, depkt_need_kf) = depacketizer.process(payload, marker);

                        if depkt_need_kf {
                            need_keyframe.store(true, Ordering::Relaxed);
                        }

                        // Send depacketized frame to pre-initialized decode thread
                        // Use try_send to avoid blocking the async RTP loop;
                        // if channel is full, drop the frame and request keyframe
                        if let Some(h264_data) = frame_data {
                            match frame_sender.try_send(h264_data) {
                                Ok(()) => {}
                                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                    // Channel full — decode thread is behind.
                                    // Request keyframe for clean restart.
                                    need_keyframe.store(true, Ordering::Relaxed);
                                }
                                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                    break; // Decode thread exited
                                }
                            }
                        }
                    }
                    Some(TrackRemoteEvent::OnEnded) => {
                        info!("[{}] Video track ended", viewer_id);
                        break;
                    }
                    None => {
                        // No packet available — brief sleep to avoid busy loop
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                    _ => {}
                }
            }

            info!("[{}] Video track packet collector ended", viewer_id);
        }));
    }
}

// ─── Clipboard message handling ──────────────────────────────────────────

fn handle_clipboard_msg(data: &[u8], callback: &Arc<dyn ViewerCallback>, peer_id: &str) {
    if data.is_empty() {
        return;
    }
    let tag = data[0];
    match tag {
        frame::CLIP_TAG_TEXT => {
            let text = String::from_utf8_lossy(&data[1..]).to_string();
            info!(
                "[{}] clipboard text received ({} bytes)",
                peer_id,
                text.len()
            );
            callback.on_clipboard_text(text);
        }
        frame::CLIP_TAG_IMAGE => {
            if data.len() < 1 + frame::CLIP_IMAGE_HDR_SIZE {
                return;
            }
            let width = u32::from_le_bytes(data[1..5].try_into().unwrap_or_default());
            let height = u32::from_le_bytes(data[5..9].try_into().unwrap_or_default());
            let rgba = &data[9..];
            info!(
                "[{}] clipboard image received: {}x{}",
                peer_id, width, height
            );
            callback.on_clipboard_image(rgba.to_vec(), width, height);
        }
        frame::CLIP_TAG_FILE_LIST => {
            // Parse file URIs from the list for clipboard sync
            let data_inner = &data[1..];
            if data_inner.len() < frame::CLIP_FILE_LIST_MIN_LEN {
                return;
            }
            let count = u16::from_le_bytes([data_inner[0], data_inner[1]]) as usize;
            let mut offset = frame::CLIP_FILE_LIST_MIN_LEN;
            let mut uris = Vec::new();
            for _ in 0..count {
                if offset + 2 > data_inner.len() {
                    break;
                }
                let name_len =
                    u16::from_le_bytes([data_inner[offset], data_inner[offset + 1]]) as usize;
                offset += 2;
                if offset + name_len > data_inner.len() {
                    break;
                }
                let name =
                    String::from_utf8_lossy(&data_inner[offset..offset + name_len]).to_string();
                offset += name_len;
                if offset + 8 > data_inner.len() {
                    break;
                }
                let _size = u64::from_le_bytes(
                    data_inner[offset..offset + 8]
                        .try_into()
                        .unwrap_or_default(),
                );
                offset += 8;
                uris.push(name);
            }
            let uris_str = uris.join("\n");
            callback.on_clipboard_file_uris(uris_str);
        }
        frame::CLIP_TAG_DONE => {
            // File transfer complete — handled at a higher level
        }
        frame::CLIP_TAG_FILE_CHUNK => {
            // Individual file chunks — handled at a higher level
        }
        _ => {
            warn!("[{}] Unknown clipboard tag: 0x{:02X}", peer_id, tag);
        }
    }
}
