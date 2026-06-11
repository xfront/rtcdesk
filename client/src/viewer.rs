//! WebRTC viewer client using webrtc-rs PeerConnection.
//!
//! **Signaling**: Uses IPC to communicate with host-service.
//! Video frames are decoded in Rust and rendered via <img> data URLs —
//! no JavaScript WebRTC bridge is needed.
//!
//! DataChannels for input and clipboard are also created and managed in Rust.
//!
//! **Architecture**:
//! Client GUI ↔ IPC (JSON-RPC) ↔ Host-Service ↔ WebSocket ↔ Signal Server

use anyhow::{Context, Result};
use bytes::BytesMut;
use log::{error, info, warn};
use shared::frame;
use shared::sdp_utils;
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

/// Manages the WebRTC PeerConnection.
///
/// Video is received via the video track, decoded in Rust, and
/// rendered as JPEG frames.  Input and clipboard use DataChannels
/// that are created and polled entirely in Rust.
///
/// **Signaling**: Uses IPC to communicate with host-service.
pub struct RtcViewer {
    viewer_id: String,
    node_id: String,
    callback: Arc<dyn ViewerCallback>,
    connected: Arc<AtomicBool>,
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
    // TODO: Add RPC-based signaling when needed
    // rpc_signaler: Option<RpcSignaler>,
}

impl RtcViewer {
    pub fn new(viewer_id: String, node_id: String, cb: Arc<dyn ViewerCallback>) -> Self {
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
                info!("[{}] Waiting for frames on channel...", vid);

                // Signal that the decoder is ready — host should wait for
                // this before streaming to avoid sending frames to an
                // unready decoder (SPS/PPS get lost or arrive too early).
                decoder_ready_flag.store(true, Ordering::Relaxed);

                // Decode ALL frames sequentially (preserves H264 reference
                // chain). Do NOT drain-to-latest — that breaks reference
                // frames and causes "reference picture missing" errors.
                // The FrameSlot always holds the latest decoded frame,
                // so the UI naturally displays the most recent one.
                loop {
                    match frame_rx.recv() {
                        Ok(h264_data) => {
                            info!("[{}] Received frame ({} bytes)", vid, h264_data.len());
                            let (result, need_kf) = decoder.decode_frame(&h264_data);
                            if need_kf {
                                need_kf_flag.store(true, Ordering::Relaxed);
                            }
                            if let Some(decoded) = result {
                                need_kf_flag.store(false, Ordering::Relaxed);
                                cb_ref.on_video_frame(&decoded.bgra_data, decoded.width, decoded.height);
                            }
                        }
                        Err(e) => {
                            // Channel closed - this means all senders were dropped
                            warn!("[{}] Frame channel closed: {:?} - decode thread exiting", vid, e);
                            break;
                        }
                    }
                }

                info!("[{}] Decode thread ended", vid);
            })
            .expect("failed to spawn decode thread");

        Self {
            viewer_id,
            node_id,
            callback: cb,
            connected: Arc::new(AtomicBool::new(false)),
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

    pub fn viewer_id(&self) -> &str {
        &self.viewer_id
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    #[allow(dead_code)]
    pub fn set_bitrate(&mut self, bitrate: Option<u32>) {
        self.bitrate = bitrate;
    }

    pub fn capturer_params(&self) -> sdp_utils::CapturerParams {
        sdp_utils::CapturerParams {
            pic_quality: Some(self.pic_quality),
            max_dimension: Some(std::cmp::max(self.width, self.height)),
            fps: Some(self.fps),
        }
    }

    /// Connect to remote host via RPC-based WebRTC signaling.
    /// 
    /// Flow:
    /// 1. Create PeerConnection with ICE servers
    /// 2. Generate Offer SDP
    /// 3. Call viewer_connect_with_offer() via RPC
    /// 4. Receive Answer SDP from host
    /// 5. Set Answer as remote description
    /// 6. ICE candidates trickle via callbacks
    pub async fn connect_rpc(
        &mut self,
        rpc_client: &crate::rpc_client::RpcClient,
        password: String,
        stun_servers: Vec<String>,
    ) -> Result<()> {
        info!("[{}] Starting RPC-based WebRTC connection", self.viewer_id);

        // Build ICE configuration (same pattern as host-service/peer.rs)
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

        // Build media engine with H.264 codec
        let mut media_engine = MediaEngine::default();
        let video_codec = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                        .to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 102,
            ..Default::default()
        };        
        info!("[{}] Registering H.264 codec...", self.viewer_id);
        media_engine.register_codec(video_codec, RtpCodecKind::Video)
            .map_err(|e| {
                error!("[{}] Failed to register codec: {}", self.viewer_id, e);
                anyhow::anyhow!("Failed to register codec: {}", e)
            })?;
        info!("[{}] H.264 codec registered", self.viewer_id);

        info!("[{}] Registering default interceptors...", self.viewer_id);
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)
            .map_err(|e| {
                error!("[{}] Failed to register interceptors: {}", self.viewer_id, e);
                anyhow::anyhow!("Failed to register interceptors: {}", e)
            })?;
        info!("[{}] Interceptors registered", self.viewer_id);

        let runtime = default_runtime()
            .ok_or_else(|| anyhow::anyhow!("no async runtime found"))?;

        // Create event handler with all required fields
        let handler = Arc::new(ViewerEventHandler::new(
            self.callback.clone(),
            self.viewer_id.clone(),
            runtime.clone(),
            self.frame_sender.clone(),
            self.need_keyframe.clone(),
            self.input_dc.clone(),
            self.clipboard_dc.clone(),
            self.running.clone(),
        ));

        // Create PeerConnection using builder pattern
        info!("[{}] Building PeerConnection...", self.viewer_id);
        
        // Discover available UDP addresses for ICE
        // Use 0.0.0.0:0 to let the system choose an available address
        let udp_addrs = vec!["0.0.0.0:0".to_string()];
        info!("[{}] Using UDP addresses for ICE: {:?}", self.viewer_id, udp_addrs);
        
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime)
            .with_udp_addrs(udp_addrs)
            .build()
            .await
            .map_err(|e| {
                error!("[{}] Failed to build peer connection: {}", self.viewer_id, e);
                anyhow::anyhow!("Failed to build peer connection: {}", e)
            })?;

        let pc: Box<dyn PeerConnection> = Box::new(pc);
        let pc: Arc<dyn PeerConnection> = Arc::from(pc);
        info!("[{}] PeerConnection created successfully", self.viewer_id);

        // Create Offer
        info!("[{}] Creating Offer SDP...", self.viewer_id);
        let offer = pc.create_offer(None).await
            .map_err(|e| {
                error!("[{}] Failed to create offer: {}", self.viewer_id, e);
                anyhow::anyhow!("Failed to create offer: {}", e)
            })?;
        info!("[{}] Created Offer SDP ({} bytes)", self.viewer_id, offer.sdp.len());

        // Set local description
        info!("[{}] Setting local description...", self.viewer_id);
        pc.set_local_description(offer.clone()).await
            .map_err(|e| {
                error!("[{}] Failed to set local description: {}", self.viewer_id, e);
                anyhow::anyhow!("Failed to set local description: {}", e)
            })?;
        info!("[{}] Set local description", self.viewer_id);

        // Call RPC to send Offer and receive Answer
        info!("[{}] Calling RPC viewer_connect_with_offer...", self.viewer_id);
        let answer = rpc_client
            .viewer_connect_with_offer(
                self.viewer_id.clone(),
                self.node_id.clone(),
                password,
                offer.sdp,
            )
            .await
            .map_err(|e| {
                error!("[{}] RPC viewer_connect_with_offer failed: {}", self.viewer_id, e);
                anyhow::anyhow!("RPC viewer_connect_with_offer failed: {}", e)
            })?;
        info!("[{}] RPC viewer_connect_with_offer returned successfully", self.viewer_id);

        if !answer.accepted {
            return Err(anyhow::anyhow!("Connection rejected: {}", answer.reason));
        }

        info!("[{}] Received Answer SDP ({} bytes)", self.viewer_id, answer.answer.len());

        // Parse Answer SDP
        let answer_sdp = RTCSessionDescription::answer(answer.answer)
            .map_err(|e| anyhow::anyhow!("Failed to parse Answer SDP: {}", e))?;

        // Set remote description
        pc.set_remote_description(answer_sdp).await?;
        info!("[{}] Set remote description (Answer)", self.viewer_id);

        // Store PeerConnection
        {
            let mut guard = self.pc.lock().await;
            *guard = Some(pc);
        }

        self.connected.store(true, Ordering::Relaxed);
        info!("[{}] WebRTC connection established via RPC", self.viewer_id);

        Ok(())
    }

    // TODO: Implement RPC-based connect method
    // The connect_ipc method has been removed as it depended on the deleted ipc_signaler
    // WebRTC connection should be established via HostCtx.rpc_client instead

    // TODO: Implement RPC-based signaling methods
    // send_answer_ipc, send_ice_candidate_ipc, and send_quality_update_ipc have been removed
    // These should be implemented using HostCtx.rpc_client when needed

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
    }

    /// Connect to host via RPC - creates Offer and sends to remote host
    /// Returns the Offer SDP to be sent via RPC
    pub async fn create_offer(
        &mut self,
        stun_servers: Vec<String>,
    ) -> Result<String> {
        info!(
            "[{}] Creating Offer for host '{}' via RPC",
            self.viewer_id, self.node_id
        );
    
        // Build PeerConnection
        let handler = Arc::new(ViewerEventHandler::new(
            self.callback.clone(),
            self.viewer_id.clone(),
            self.runtime.clone(),
            self.frame_sender.clone(),
            self.need_keyframe.clone(),
            self.input_dc.clone(),
            self.clipboard_dc.clone(),
            self.running.clone(),
        ));
    
        let (pc, _media_engine) = build_viewer_peer_connection(
            &stun_servers,
            handler,
            self.runtime.clone(),
        ).await?;
    
        // Create Offer
        let offer = pc.create_offer(None).await
            .context("failed to create offer")?;
            
        pc.set_local_description(offer.clone()).await
            .context("failed to set local description")?;
    
        // Wait for ICE gathering to start so that ufrag/pwd are included in the SDP
        // This is important because the host needs ICE credentials to set remote description
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
        info!("[{}] Created Offer (SDP length: {})", self.viewer_id, offer.sdp.len());
    
        // Store PC for later use
        *self.pc.lock().await = Some(pc);
    
        // Return Offer SDP to be sent via RPC
        Ok(offer.sdp)
    }

    /// Set remote description with Answer SDP from host
    pub async fn set_answer(&self, answer_sdp: String) -> Result<()> {
        let pc_guard = self.pc.lock().await;
        if let Some(peer) = pc_guard.as_ref() {
            let answer = RTCSessionDescription::answer(answer_sdp)
                .context("failed to create answer description")?;
            peer.set_remote_description(answer).await
                .context("failed to set remote description")?;
            info!("[{}] Set Answer SDP from host", self.viewer_id);
            Ok(())
        } else {
            anyhow::bail!("PeerConnection not initialized");
        }
    }

    /// Add ICE candidate from remote host
    /// Note: webrtc-rs may not support add_ice_candidate - using bundled ICE approach
    pub async fn add_ice_candidate(
        &self,
        candidate: String,
        _sdp_mid: Option<String>,
        _sdp_mline_index: Option<u16>,
    ) -> Result<()> {
        // Log the candidate - bundled ICE approach includes these in SDP
        info!(
            "[{}] Received ICE candidate from host: {}",
            self.viewer_id, candidate
        );
        // TODO: If webrtc-rs supports add_ice_candidate, implement it here
        // For now, rely on bundled ICE in the initial SDP exchange
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

impl ViewerEventHandler {
    pub fn new(
        callback: Arc<dyn ViewerCallback>,
        viewer_id: String,
        runtime: Arc<dyn Runtime>,
        frame_sender: std::sync::mpsc::SyncSender<Vec<u8>>,
        need_keyframe: Arc<AtomicBool>,
        input_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
        clipboard_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
        running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            viewer_id,
            callback,
            running,
            runtime,
            input_dc,
            clipboard_dc,
            frame_sender,
            need_keyframe,
            decoder_ready: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for ViewerEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        // Forward ICE candidates via callback for trickle ICE
        if let Ok(init) = event.candidate.to_json() {
            if !init.candidate.is_empty() {
                info!(
                    "[{}] ICE candidate generated: {}",
                    self.viewer_id, init.candidate
                );
                // Note: ICE candidates are bundled in the Offer SDP during set_local_description
                // For trickle ICE, we would forward them here via RPC
                // Currently using bundled ICE approach - all candidates in initial SDP
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
