//! WebRTC peer connection management for RTCDesk.
//!
//! Handles establishing and maintaining WebRTC connections,
//! screen frame transmission over data channels, and input event relay.
//!
//! Uses webrtc-rs 0.20 alpha API with sans-IO architecture.

use anyhow::{Context, Result};
use bytes::BytesMut;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData, common::RustImage};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse};
use log::{debug, error, info, warn};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit};

// ── Key code constants ─────────────────────────────────────────────────────
use frame::{
    VK_ALT, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, VK_V,
};
const ASCII_PRINTABLE_MIN: u32 = 0x21;
const ASCII_PRINTABLE_MAX: u32 = 0x7E;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfiguration,
    RTCConfigurationBuilder, RTCIceConnectionState, RTCIceServer, RTCPeerConnectionIceEvent,
    RTCPeerConnectionState, RTCSdpType, RTCSessionDescription,
};
use shared::{frame, signal};
use shared::signal::SignalMessage;
use crate::capture::ScreenCapturer;

/// Single global `enigo` instance — re‑using the same X11/Wayland connection is **required**
/// so that modifier keys (Ctrl/Shift/Alt) remain active when the next key arrives.
static ENIGO: LazyLock<Mutex<Enigo>> =
    LazyLock::new(|| Mutex::new(Enigo::new(&enigo::Settings::default()).expect("enigo init")));

/// Track whether Ctrl is currently held via enigo so `dispatch_key` can detect
/// Ctrl+V and delegate the paste via clipboard-rs.
static CTRL_HELD: AtomicBool = AtomicBool::new(false);

/// Last plain-text clipboard content received from the viewer.  Used by the
/// Ctrl+V handler to re-populate the clipboard right before triggering paste.
static LAST_RECEIVED_TEXT: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

/// Compute a stable hash for clipboard text, used to detect sync loops.
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

/// Shared state for clipboard text sync loop prevention.
struct ClipboardSyncState {
    last_received_hash: u64,
}

/// Shared state for clipboard image sync loop prevention.
struct ClipboardImageSyncState {
    last_received_hash: u64,
}

static CLIP_SYNC: LazyLock<Mutex<ClipboardSyncState>> = LazyLock::new(|| {
    Mutex::new(ClipboardSyncState {
        last_received_hash: 0,
    })
});

static CLIP_IMAGE_SYNC: LazyLock<Mutex<ClipboardImageSyncState>> = LazyLock::new(|| {
    Mutex::new(ClipboardImageSyncState {
        last_received_hash: 0,
    })
});

/// Discover local IP addresses for ICE host candidates.
/// Falls back to 0.0.0.0 binding if no local IP is found.
pub fn discover_udp_addrs() -> Vec<String> {
    match local_ip_address::local_ip() {
        Ok(ip) => vec![format!("{}:0", ip)],
        Err(_) => vec!["0.0.0.0:0".to_string()],
    }
}

/// Handler that bridges WebRTC events to the signaling channel.
#[derive(Clone)]
struct SignalingEventHandler {
    /// Sender for forwarding ICE candidates as signaling messages.
    ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    /// Our peer ID.
    peer_id: String,
    /// Notified when ICE connection is established.
    connection_ready: Arc<tokio::sync::Notify>,
    /// Set to false when the peer disconnects; streaming loop checks this.
    running: Arc<AtomicBool>,
}

impl SignalingEventHandler {
    fn new(
        ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
        peer_id: String,
        connection_ready: Arc<tokio::sync::Notify>,
        running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            ice_tx,
            peer_id,
            connection_ready,
            running,
        }
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SignalingEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(init) => {
                if init.candidate.is_empty() {
                    // End-of-candidates indicator; not an error.
                    info!("[{}] ICE candidate gathering complete", self.peer_id);
                    return;
                }
                let msg = SignalMessage::IceCandidate {
                    from: self.peer_id.clone(),
                    to: String::new(), // Set by sender
                    candidate: init.candidate,
                    sdp_mid: init.sdp_mid,
                    sdp_mline_index: init.sdp_mline_index,
                };
                if let Err(e) = self.ice_tx.send(msg) {
                    error!("[{}] ICE candidate send error: {}", self.peer_id, e);
                }
                info!("[{}] ICE candidate generated", self.peer_id);
            }
            Err(e) => {
                error!(
                    "[{}] ICE candidate to_json() failed: {}",
                    self.peer_id, e
                );
            }
        }
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        info!("[{}] ICE connection state: {:?}", self.peer_id, state);
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("[{}] Connection state: {:?}", self.peer_id, state);
        if state == RTCPeerConnectionState::Connected {
            self.connection_ready.notify_waiters();
        }
        if state == RTCPeerConnectionState::Disconnected
            || state == RTCPeerConnectionState::Failed
            || state == RTCPeerConnectionState::Closed
        {
            self.running.store(false, Ordering::Relaxed);
        }
    }

    async fn on_data_channel(&self, _channel: Arc<dyn DataChannel>) {
        info!("[{}] Remote data channel received", self.peer_id);
    }
}
/// Manages a WebRTC peer connection for desktop sharing.
pub struct RtcPeer {
    /// The underlying WebRTC peer connection.
    peer_connection: Arc<dyn PeerConnection>,
    /// Our peer ID.
    peer_id: String,
    /// Screen capturer for desktop frames.
    capturer: Option<Arc<ScreenCapturer>>,
    /// Pre-created data channel for frame streaming (before offer).
    frame_channel: Option<Arc<dyn DataChannel>>,
    /// Channel for sending ICE candidate signaling messages.
    ice_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
    /// Receiver end — call `take_ice_receiver()` to drain and forward candidates.
    ice_rx: Option<tokio::sync::mpsc::UnboundedReceiver<SignalMessage>>,
    /// Notified when ICE connection is established.
    connection_ready: Arc<tokio::sync::Notify>,
    /// Notified when the pre-created data channel is fully open.
    dc_ready: Arc<tokio::sync::Notify>,
    /// Set to false by the event handler when peer disconnects.
    running: Arc<AtomicBool>,
    /// Current target FPS (updated dynamically via QualityUpdate).
    fps: Arc<AtomicU32>,
    /// Guard against calling pc.close() more than once (prevents double-close panic/crash).
    pc_closed: Arc<AtomicBool>,
}

impl RtcPeer {
    /// Create a new RTC peer (host mode) with desktop sharing capability.
    pub async fn new_host(peer_id: &str, stun_servers: &[String]) -> Result<Self> {
        let config = build_configuration(stun_servers);
        let (ice_tx, ice_rx) = tokio::sync::mpsc::unbounded_channel();
        let connection_ready = Arc::new(tokio::sync::Notify::new());
        let dc_ready = Arc::new(tokio::sync::Notify::new());
        let running = Arc::new(AtomicBool::new(true));

        let handler = Arc::new(SignalingEventHandler::new(
            ice_tx.clone(),
            peer_id.to_string(),
            connection_ready.clone(),
            running.clone(),
        ));

        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_handler(handler)
            .with_udp_addrs(discover_udp_addrs())
            .build()
            .await
            .context("failed to build peer connection")?;

        // Box it and wrap in Arc
        let pc: Box<dyn PeerConnection> = Box::new(pc);
        let pc = Arc::from(pc);

        info!("[{}] Host peer connection created", peer_id);

        Ok(Self {
            peer_connection: pc,
            peer_id: peer_id.to_string(),
            capturer: None,
            frame_channel: None,
            ice_tx,
            ice_rx: Some(ice_rx),
            connection_ready,
            dc_ready,
            running,
            fps: Arc::new(AtomicU32::new(frame::DEFAULT_FPS)),
            pc_closed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Attach a screen capturer (host only).
    pub fn attach_capturer(&mut self, capturer: Arc<ScreenCapturer>) -> Result<()> {
        self.capturer = Some(capturer);
        Ok(())
    }

    /// Return a clone of the running flag so external tasks
    /// (e.g. input reader) can check connection liveness.
    pub fn running_flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Extract capturer parameters from a remote SDP (Offer or Answer)
    /// and apply them to the attached `ScreenCapturer`.
    pub fn apply_capturer_params_from_sdp(&self, sdp: &str) {
        let params = signal::extract_capturer_params_from_sdp(sdp);
        if params.pic_quality.is_none() && params.max_dimension.is_none() {
            return; // No custom attributes → keep defaults
        }
        if let Some(ref c) = self.capturer {
            if let Some(q) = params.pic_quality {
                let q8 = q.clamp(0, 100) as u8;
                info!(
                    "[{}] Applying pic_quality={} from remote SDP",
                    self.peer_id, q8
                );
                c.set_quality(q8);
            }
            if let Some(d) = params.max_dimension {
                info!(
                    "[{}] Applying max_dimension={} from remote SDP",
                    self.peer_id, d
                );
                c.set_max_dimension(d);
            }
            if let Some(f) = params.fps {
                info!(
                    "[{}] Remote requested fps={} (applied at stream start)",
                    self.peer_id, f
                );
            }
        }
    }

    /// Update quality, resolution, and FPS dynamically during an active session.
    /// Atomic writes ensure the stream loop picks up the changes immediately.
    pub fn update_quality(&self, quality: u32, max_dimension: u32, fps: u32) {
        if let Some(ref c) = self.capturer {
            c.set_quality(quality.clamp(0, 100) as u8);
            c.set_max_dimension(max_dimension);
            self.fps.store(fps.max(1), Ordering::Relaxed);
            info!(
                "[{}] Quality updated: quality={} dim={} fps={}",
                self.peer_id, quality, max_dimension, fps
            );
        } else {
            warn!("[{}] No capturer; quality update ignored", self.peer_id);
        }
    }

    /// Read the fps hint from a remote SDP. Used to override the default fps
    /// when the remote side requests a specific frame rate.
    pub fn fps_from_remote_sdp(sdp: &str, default_fps: u32) -> u32 {
        signal::extract_capturer_params_from_sdp(sdp)
            .fps
            .unwrap_or(default_fps)
    }

    /// Take the ICE candidate receiver for forwarding to the signaling server.
    /// Must be called before ICE candidates are generated (i.e. before create_offer).
    /// Returns `None` if already taken.
    pub fn take_ice_receiver(
        &mut self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<SignalMessage>> {
        self.ice_rx.take()
    }

    /// Create a WebRTC offer and return the SDP.
    pub async fn create_offer(&self) -> Result<RTCSessionDescription> {
        let offer = self.peer_connection.create_offer(None).await?;
        self.peer_connection
            .set_local_description(offer.clone())
            .await?;

        // ICE gathering is automatic - we'll receive candidates via handler
        // The local description is set; ICE candidates flow through the handler

        info!("[{}] Created offer", self.peer_id);
        Ok(offer)
    }

    /// Create a WebRTC answer and return the SDP.
    pub async fn create_answer(&self) -> Result<RTCSessionDescription> {
        let answer = self.peer_connection.create_answer(None).await?;
        self.peer_connection
            .set_local_description(answer.clone())
            .await?;

        info!("[{}] Created answer", self.peer_id);
        Ok(answer)
    }

    /// Set the remote SDP description.
    pub async fn set_remote_description(&self, sdp: &str, sdp_type: RTCSdpType) -> Result<()> {
        let desc = match sdp_type {
            RTCSdpType::Offer => RTCSessionDescription::offer(sdp.to_string())?,
            RTCSdpType::Answer => RTCSessionDescription::answer(sdp.to_string())?,
            _ => RTCSessionDescription::offer(sdp.to_string())?,
        };
        self.peer_connection.set_remote_description(desc).await?;
        Ok(())
    }

    /// Add a remote ICE candidate.
    pub async fn add_ice_candidate(
        &self,
        candidate: &str,
        sdp_mid: &str,
        sdp_mline_index: u16,
    ) -> Result<()> {
        let candidate_init = webrtc::peer_connection::RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: if sdp_mid.is_empty() {
                None
            } else {
                Some(sdp_mid.to_string())
            },
            sdp_mline_index: Some(sdp_mline_index),
            username_fragment: None,
            url: None,
        };
        self.peer_connection
            .add_ice_candidate(candidate_init)
            .await?;
        Ok(())
    }

    /// Get our peer ID.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Create a data channel for sending desktop frames.
    pub async fn create_frame_channel(&self, label: &str) -> Result<Arc<dyn DataChannel>> {
        let dc = self
            .peer_connection
            .create_data_channel(label, Some(RTCDataChannelInit::default()))
            .await?;
        info!("[{}] Created data channel: {}", self.peer_id, label);
        Ok(dc)
    }

    /// Set a pre-created frame channel. Must be created before `create_offer`
    /// so that the offer SDP includes the data channel media section.
    pub fn set_frame_channel(&mut self, dc: Arc<dyn DataChannel>) {
        let dc_ready = self.dc_ready.clone();
        let dc_clone = dc.clone();
        // Poll for OnOpen event to detect when the data channel is fully
        // open (DTLS/SCTP negotiation complete). on_open() is not available
        // on Arc<dyn DataChannel> in webrtc 0.20 alpha, so we poll instead.
        tokio::spawn(async move {
            let mut opened = false;
            loop {
                match dc_clone.poll().await {
                    Some(DataChannelEvent::OnOpen) => {
                        if !opened {
                            info!("Data channel fully open (DTLS/SCTP complete)");
                            dc_ready.notify_waiters();
                            opened = true;
                        }
                        // Keep polling to drain OnBufferedAmountLow etc.
                        // and prevent the driver's event queue from overflowing.
                    }
                    Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => {
                        warn!("Data channel closed/errored");
                        break;
                    }
                    None => {
                        tokio::time::sleep(Duration::from_millis(frame::DC_OPEN_POLL_MS)).await;
                    }
                    _ => {
                        // Drain OnBufferedAmountLow and other events.
                    }
                }
            }
        });
        self.frame_channel = Some(dc);
    }

    /// Start streaming desktop frames over the data channel.
    /// Waits for the ICE connection to be established before creating the data channel.
    pub async fn start_streaming(&self, channel_label: &str, initial_fps: u32) -> Result<()> {
        let capturer = self
            .capturer
            .as_ref()
            .context("no capturer attached")?
            .clone();

        // Store initial FPS; future QualityUpdate messages will update this atomically
        self.fps
            .store(initial_fps.max(1), Ordering::Relaxed);

        // Wait for ICE connection to be established (with timeout)
        tokio::select! {
            _ = self.connection_ready.notified() => {
                info!("[{}] ICE connection established, starting stream", self.peer_id);
            }
            _ = tokio::time::sleep(Duration::from_secs(frame::ICE_TIMEOUT_SECS)) => {
                warn!("[{}] Timed out waiting for ICE connection, starting anyway", self.peer_id);
            }
        }

        // Use pre-created channel if available, otherwise create one
        let dc = match &self.frame_channel {
            Some(dc) => {
                info!(
                    "[{}] Using pre-created data channel: {}",
                    self.peer_id, channel_label
                );
                dc.clone()
            }
            None => self.create_frame_channel(channel_label).await?,
        };

        // Wait for the data channel to be fully open (DTLS/SCTP negotiation complete)
        if self.frame_channel.is_some() {
            tokio::select! {
                _ = self.dc_ready.notified() => {
                    info!("[{}] Data channel open, ready to stream", self.peer_id);
                }
                _ = tokio::time::sleep(Duration::from_secs(frame::DC_OPEN_TIMEOUT_SECS)) => {
                    warn!("[{}] Timed out waiting for data channel to open, streaming anyway", self.peer_id);
                }
            }
        }

        let dynamic_fps = self.fps.clone();
        info!(
            "[{}] Starting desktop stream at {} fps",
            self.peer_id, initial_fps
        );

        let peer_id = self.peer_id.clone();
        let running = self.running.clone();
        let pc = self.peer_connection.clone();
        let pc_closed = self.pc_closed.clone();
        tokio::spawn(async move {
            run_stream_loop(capturer, dc, dynamic_fps, running, pc, pc_closed).await;
            info!("[{}] Streaming loop ended", peer_id);
        });

        Ok(())
    }

    /// Close the peer connection.
    /// Sets `running = false` first so spawned tasks (input/clipboard
    /// readers, stream loop) detect the shutdown and exit gracefully.
    /// Uses an atomic guard to prevent double-close.
    pub async fn close(&self) -> Result<()> {
        // Prevent double-close (can happen when both the stream loop and the
        // host cleanup try to close the same peer connection concurrently).
        if self.pc_closed.swap(true, Ordering::SeqCst) {
            info!(
                "[{}] Peer connection already closed, skipping",
                self.peer_id
            );
            return Ok(());
        }
        // Signal all spawned tasks to stop BEFORE closing the peer connection,
        // so they don't keep polling a closing data channel.
        self.running.store(false, Ordering::Relaxed);
        // Close with a timeout to prevent hanging on a stuck peer connection.
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.peer_connection.close(),
        )
        .await
        {
            Ok(Ok(())) => {
                info!("[{}] Peer connection closed", self.peer_id);
                Ok(())
            }
            Ok(Err(e)) => {
                error!("[{}] Peer connection close error: {}", self.peer_id, e);
                Err(anyhow::anyhow!("close error: {}", e))
            }
            Err(_) => {
                warn!(
                    "[{}] Timed out waiting for peer connection to close",
                    self.peer_id
                );
                Ok(())
            }
        }
    }
}

/// Core streaming loop: capture frames, chunk, send over data channel.
/// FPS is read atomically each iteration so quality updates take effect immediately.
async fn run_stream_loop(
    capturer: Arc<ScreenCapturer>,
    dc: Arc<dyn DataChannel>,
    fps: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
    pc: Arc<dyn PeerConnection>,
    pc_closed: Arc<AtomicBool>,
) {
    let frame_counter = Arc::new(AtomicU32::new(0));
    loop {
        if !running.load(Ordering::Relaxed) {
            info!("Peer disconnected, stopping stream");
            break;
        }
        let frame_start = tokio::time::Instant::now();

        let current_fps = fps.load(Ordering::Relaxed).max(1);
        let interval = Duration::from_millis((1000 / current_fps) as u64);

        let c = capturer.clone();
        let frame_result = tokio::task::spawn_blocking(move || c.capture_frame()).await;
        let frame = match frame_result {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                warn!("Frame capture error: {}", e);
                continue;
            }
            Err(join_err) => {
                error!("Capture thread panicked: {}", join_err);
                break;
            }
        };

        let fid = frame_counter.fetch_add(1, Ordering::Relaxed);
        let mut payload = Vec::with_capacity(frame::FRAME_HDR_SIZE + frame.data.len());
        // Extended header: [orig_w:u32 BE][orig_h:u32 BE][frame_w:u32 BE][frame_h:u32 BE]
        payload.extend_from_slice(&frame.orig_width.to_be_bytes());
        payload.extend_from_slice(&frame.orig_height.to_be_bytes());
        payload.extend_from_slice(&frame.width.to_be_bytes());
        payload.extend_from_slice(&frame.height.to_be_bytes());
        payload.extend_from_slice(&frame.data);

        let chunks = frame::chunk_frame(fid, &payload);
        let total = chunks.len();
        let mut send_ok = true;
        for (i, chunk) in chunks.into_iter().enumerate() {
            match dc.send(BytesMut::from(&chunk[..])).await {
                Ok(()) => {}
                Err(e) => {
                    error!("Failed to send frame chunk {}/{}: {}", i + 1, total, e);
                    send_ok = false;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(frame::INTER_CHUNK_DELAY_MS)).await;
        }
        if !send_ok {
            break;
        }

        let elapsed = frame_start.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }
    // Only close if not already closed (e.g. by RtcPeer::close() from the
    // host cleanup path). Data channel send failures also end up here and
    // should trigger a graceful PC close.
    if !pc_closed.swap(true, Ordering::SeqCst) {
        running.store(false, Ordering::Relaxed);
        info!("Stream loop closing peer connection");
        if let Err(e) = pc.close().await {
            error!("Stream loop peer_connection.close() error: {}", e);
        }
    }
}

/// Build a WebRTC configuration with STUN servers.
/// Falls back to a well-known public STUN server when none are configured,
/// matching the viewer's behavior so both sides can generate srflx candidates
/// for NAT traversal.
fn build_configuration(stun_servers: &[String]) -> RTCConfiguration {
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

    RTCConfigurationBuilder::new()
        .with_ice_servers(ice_servers)
        .build()
}

// ── Remote input handling (host receives viewer input events) ───────────

/// Spawn a task that polls the `"input"` data channel and dispatches
/// received mouse / keyboard events to the OS via `enigo`.
pub fn spawn_input_reader(dc: Arc<dyn DataChannel>, running: Arc<AtomicBool>, peer_id: String) {
    tokio::spawn(async move {
        info!("[{}] Input reader started", peer_id);
        while running.load(Ordering::Relaxed) {
            match dc.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    dispatch_input(&msg.data, &peer_id);
                }
                Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => {
                    info!("[{}] Input channel closed", peer_id);
                    break;
                }
                None => {
                    tokio::time::sleep(Duration::from_millis(frame::INPUT_POLL_MS)).await;
                }
                _ => {}
            }
        }
        info!("[{}] Input reader ended", peer_id);
    });
}

/// Route a binary input payload to the correct handler.
fn dispatch_input(data: &[u8], peer_id: &str) {
    if data.is_empty() {
        return;
    }
    let tag = data[0];
    info!(
        "[{}] dispatch_input: tag=0x{:02X} len={}",
        peer_id, tag, data.len()
    );
    match tag {
        frame::TAG_MOUSE => dispatch_mouse(data, peer_id),
        frame::TAG_KEY => dispatch_key(data, peer_id),
        frame::TAG_WHEEL => dispatch_wheel(data, peer_id),
        _ => warn!("[{}] Unknown input event type: 0x{:02X}", peer_id, data[0]),
    }
}

/// Parse a mouse event and simulate it via `enigo`.
/// Format: `[0x01][x:f64le][y:f64le][button:u8][pressed:u8]`
fn dispatch_mouse(data: &[u8], peer_id: &str) {
    if data.len() < frame::MOUSE_EVENT_SIZE {
        warn!("[{}] Mouse event too short: {} bytes", peer_id, data.len());
        return;
    }
    let x = f64::from_le_bytes(
        data[frame::MOUSE_X_OFF..frame::MOUSE_Y_OFF]
            .try_into()
            .unwrap_or_default(),
    );
    let y = f64::from_le_bytes(
        data[frame::MOUSE_Y_OFF..frame::MOUSE_BTN_OFF]
            .try_into()
            .unwrap_or_default(),
    );
    let button = data[frame::MOUSE_BTN_OFF];
    let pressed = data[frame::MOUSE_PRESSED_OFF] == 1;

    info!(
        "[{}] Mouse: ({:.0}, {:.0}) btn={} {}",
        peer_id,
        x,
        y,
        button,
        if pressed { "down" } else { "up" }
    );

    match ENIGO.lock() {
        Ok(ref mut enigo) => {
            // Move to target position (absolute coordinates)
            if let Err(e) = enigo.move_mouse(x as i32, y as i32, Coordinate::Abs) {
                error!("[{}] enigo.move_mouse error: {}", peer_id, e);
            }

            // Map button code to enigo Button
            let btn = match button {
                frame::BTN_LEFT => Button::Left,
                frame::BTN_MIDDLE => Button::Middle,
                frame::BTN_RIGHT => Button::Right,
                frame::BTN_MOVE => return, // mouse-move only, no button action
                _ => {
                    warn!("[{}] Unknown mouse button: {}", peer_id, button);
                    return;
                }
            };

            let dir = if pressed {
                Direction::Press
            } else {
                Direction::Release
            };
            if let Err(e) = enigo.button(btn, dir) {
                error!("[{}] enigo.button({:?}, {:?}) error: {}", peer_id, btn, dir, e);
            }
        }
        Err(e) => {
            error!("[{}] Failed to lock enigo: {}", peer_id, e);
        }
    }
}

/// Parse a wheel event and simulate it via `enigo`.
/// Format: `[0x03][dx:i32le][dy:i32le]`
fn dispatch_wheel(data: &[u8], peer_id: &str) {
    if data.len() < frame::WHEEL_EVENT_SIZE {
        warn!("[{}] Wheel event too short: {} bytes", peer_id, data.len());
        return;
    }
    let dx = i32::from_le_bytes(
        data[frame::WHEEL_DX_OFF..frame::WHEEL_DY_OFF]
            .try_into()
            .unwrap_or_default(),
    );
    let dy = i32::from_le_bytes(
        data[frame::WHEEL_DY_OFF..frame::WHEEL_EVENT_SIZE]
            .try_into()
            .unwrap_or_default(),
    );

    debug!(
        "[{}] Wheel: dx={} dy={}",
        peer_id, dx, dy
    );

    match ENIGO.lock() {
        Ok(ref mut enigo) => {
            if let Err(e) = enigo.scroll(dy, Axis::Vertical) {
                error!("[{}] enigo.scroll(vertical) error: {}", peer_id, e);
            }
            if let Err(e) = enigo.scroll(dx, Axis::Horizontal) {
                error!("[{}] enigo.scroll(horizontal) error: {}", peer_id, e);
            }
        }
        Err(e) => {
            error!("[{}] Failed to lock enigo for wheel: {}", peer_id, e);
        }
    }
}

/// Parse a key event and simulate it via `enigo`.
/// Format: `[0x02][key_code:u32le][pressed:u8]`
fn dispatch_key(data: &[u8], peer_id: &str) {
    if data.len() < frame::KEY_EVENT_SIZE {
        warn!("[{}] Key event too short: {} bytes", peer_id, data.len());
        return;
    }
    let key_code = u32::from_le_bytes(
        data[frame::KEY_CODE_OFF..frame::KEY_PRESSED_OFF]
            .try_into()
            .unwrap_or_default(),
    );
    let pressed = data[frame::KEY_PRESSED_OFF] == 1;

    let raw_bytes: Vec<String> = data.iter().map(|b| format!("{:02X}", b)).collect();
    info!(
        "[{}] Key RAW=[{}] code=0x{:X} (dec={}) {}",
        peer_id,
        raw_bytes.join(" "),
        key_code,
        key_code,
        if pressed { "DOWN" } else { "UP" }
    );

    // ── Ctrl state tracking ───────────────────────────────────────────
    if key_code == VK_CONTROL {
        CTRL_HELD.store(pressed, Ordering::Relaxed);
    }

    // ── Ctrl+V → re-populate clipboard then forward key ─────────────────
    // enigo cannot synthesise Ctrl+Unicode at the OS level on some platforms,
    // so we ensure the clipboard still holds the last received text, then
    // let enigo forward the Ctrl+V key combination normally.
    if pressed && key_code == VK_V && CTRL_HELD.load(Ordering::Relaxed) {
        info!("[{}] Ctrl+V detected – ensuring clipboard is populated", peer_id);
        if let Ok(text) = LAST_RECEIVED_TEXT.lock() {
            if !text.is_empty() {
                set_clipboard_text(&text, peer_id);
            }
        }
        // Fall through: let enigo forward the key event normally
    }

    match ENIGO.lock() {
        Ok(ref mut enigo) => {
            let key = map_key_code(key_code);
            let dir = if pressed {
                Direction::Press
            } else {
                Direction::Release
            };
            info!("[{}] Key mapped: {:?} {:?}", peer_id, key, dir);
            match enigo.key(key, dir) {
                Ok(_) => info!("[{}] enigo.key({:?}, {:?}) OK", peer_id, key, dir),
                Err(e) => error!("[{}] enigo.key({:?}, {:?}) ERR: {}", peer_id, key, dir, e),
            }
        }
        Err(e) => {
            error!("[{}] Failed to lock enigo: {}", peer_id, e);
        }
    }
}

/// Map a raw key code to an `enigo` Key.
///
/// Key codes are expected to follow the same scheme as `composeKeyToUnicode`
/// on the Kotlin side: modifier codes use psuedo-VK values, direct-printable
/// characters use their Unicode code-point, and a handful of named keys use
/// well-known scan-code-ish values.
fn map_key_code(code: u32) -> Key {
    match code {
        VK_SHIFT => Key::Shift,
        VK_CONTROL => Key::Control,
        VK_ALT => Key::Alt,

        VK_BACK => Key::Backspace,
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Return,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_LEFT => Key::LeftArrow,
        VK_UP => Key::UpArrow,
        VK_RIGHT => Key::RightArrow,
        VK_DOWN => Key::DownArrow,
        VK_DELETE => Key::Delete,

        c if (ASCII_PRINTABLE_MIN..=ASCII_PRINTABLE_MAX).contains(&c) =>
            Key::Unicode(char::from_u32(c).unwrap_or('?')),

        _ => Key::Unicode(char::from_u32(code).unwrap_or('\0')),
    }
}

// ── Clipboard file sync (host receives viewer clipboard/file data) ─

struct ClipState {
    files: Vec<(PathBuf, File, u64)>, // (path, handle, expected_size)
    dir: PathBuf,
}

/// Spawn a task that polls the `"clipboard"` data channel and handles
/// incoming file / text clipboard data from the viewer.
pub fn spawn_clipboard_reader(dc: Arc<dyn DataChannel>, running: Arc<AtomicBool>, peer_id: String) {
    tokio::spawn(async move {
        info!("[{}] Clipboard reader started", peer_id);
        let mut state: Option<ClipState> = None;
        while running.load(Ordering::Relaxed) {
            match dc.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    if !msg.data.is_empty() {
                        state = handle_clipboard_msg(&msg.data, state, &peer_id);
                    }
                }
                Some(DataChannelEvent::OnClose) | Some(DataChannelEvent::OnError) => {
                    info!("[{}] Clipboard channel closed", peer_id);
                    break;
                }
                None => {
                    tokio::time::sleep(Duration::from_millis(frame::CLIPBOARD_POLL_MS)).await;
                }
                _ => {}
            }
        }
        info!("[{}] Clipboard reader ended", peer_id);
    });
}

fn handle_clipboard_msg(data: &[u8], state: Option<ClipState>, peer_id: &str) -> Option<ClipState> {
    match data[0] {
        frame::CLIP_TAG_TEXT => {
            // Text clipboard from viewer
            let text = String::from_utf8_lossy(&data[1..]).to_string();
            let hash = text_hash(&text);
            info!(
                "[{}] clipboard text received from viewer ({} bytes)",
                peer_id,
                text.len()
            );
            // Cache for Ctrl+V paste.
            if let Ok(mut c) = LAST_RECEIVED_TEXT.lock() {
                *c = text.clone();
            }
            set_clipboard_text(&text, peer_id);
            // Record hash so the writer thread won't echo this back to the viewer.
            if let Ok(mut sync) = CLIP_SYNC.lock() {
                sync.last_received_hash = hash;
            }
            state
        }
        frame::CLIP_TAG_IMAGE => {
            // Image clipboard from viewer: [width:u32le][height:u32le][rgba...]
            if data.len() < 1 + frame::CLIP_IMAGE_HDR_SIZE {
                warn!("[{}] Image clipboard msg too short: {} bytes", peer_id, data.len());
                return state;
            }
            let width = u32::from_le_bytes(data[1..5].try_into().unwrap_or_default());
            let height = u32::from_le_bytes(data[5..9].try_into().unwrap_or_default());
            let rgba = &data[9..];
            info!(
                "[{}] clipboard image received from viewer: {}x{} ({} bytes)",
                peer_id, width, height, rgba.len()
            );
            let hash = bytes_hash(rgba);
            set_clipboard_image(rgba, width, height, peer_id);
            if let Ok(mut sync) = CLIP_IMAGE_SYNC.lock() {
                sync.last_received_hash = hash;
            }
            state
        }
        frame::CLIP_TAG_FILE_LIST => handle_file_list(&data[1..], peer_id),
        frame::CLIP_TAG_FILE_CHUNK => handle_file_chunk(&data[1..], state, peer_id),
        frame::CLIP_TAG_DONE => {
            handle_transfer_complete(state, peer_id);
            None
        }
        _ => {
            warn!(
                "[{}] Unknown clipboard msg type: 0x{:02X}",
                peer_id, data[0]
            );
            state
        }
    }
}

/// Parse file list: [count:u16le][(name_len:u16le, name:utf8, size:u64le)*]
fn handle_file_list(data: &[u8], peer_id: &str) -> Option<ClipState> {
    if data.len() < frame::CLIP_FILE_LIST_MIN_LEN {
        warn!("[{}] File list too short", peer_id);
        return None;
    }
    let count = u16::from_le_bytes([data[0], data[1]]) as usize;
    let mut offset = frame::CLIP_FILE_LIST_MIN_LEN;
    let mut files: Vec<(PathBuf, File, u64)> = Vec::new();

    // Create temp directory for received files
    let dir = std::env::temp_dir().join("rtcdesk-clipboard");
    if let Err(e) = fs::create_dir_all(&dir) {
        error!("[{}] Failed to create temp dir {}: {}", peer_id, dir.display(), e);
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

        let fname = PathBuf::from(&name)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("file_{}", files.len()));
        let path = dir.join(&fname);

        match File::create(&path) {
            Ok(f) => {
                info!("[{}] Receiving file: {} ({} bytes)", peer_id, fname, size);
                files.push((path, f, size));
            }
            Err(e) => {
                error!("[{}] Cannot create file {}: {}", peer_id, fname, e);
            }
        }
    }

    if files.is_empty() {
        None
    } else {
        Some(ClipState { files, dir })
    }
}

/// Write chunk(s) to the correct file(s).
/// Format per chunk: [file_index:u16le][offset:u64le][len:u32le][data]
/// Single message may contain multiple concatenated chunks; iterate through all.
fn handle_file_chunk(data: &[u8], state: Option<ClipState>, peer_id: &str) -> Option<ClipState> {
    let mut s = state?;
    let mut offset = 0usize;
    let hdr = frame::CLIP_CHUNK_HDR_SIZE;
    while offset + hdr <= data.len() {
        let idx = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        let off = u64::from_le_bytes(data[offset + 2..offset + 10].try_into().unwrap_or_default());
        let len = u32::from_le_bytes(
            data[offset + 10..offset + hdr].try_into().unwrap_or_default(),
        ) as usize;
        let header_end = offset + hdr;
        if data.len() < header_end + len {
            warn!("[{}] Chunk truncated at offset {}", peer_id, offset);
            break;
        }
        let chunk_data = &data[header_end..header_end + len];

        if idx < s.files.len() {
            let file = &mut s.files[idx].1;
            if let Err(e) = file.seek(SeekFrom::Start(off)) {
                error!("[{}] Seek error: {}", peer_id, e);
            } else if let Err(e) = file.write_all(chunk_data) {
                error!("[{}] Write error: {}", peer_id, e);
            }
        }
        offset = header_end + len;
    }
    Some(s)
}

/// All chunks received — set clipboard to file URIs
fn handle_transfer_complete(state: Option<ClipState>, peer_id: &str) {
    let s = match state {
        Some(s) => s,
        None => return,
    };

    // Build path list for clipboard-rs
    let paths: Vec<String> = s
        .files
        .iter()
        .map(|(path, _, _)| path.to_string_lossy().to_string())
        .collect();

    info!(
        "[{}] Transfer complete: {} files in {}",
        peer_id,
        s.files.len(),
        s.dir.display()
    );

    // Drop file handles before setting clipboard (avoids lock issues on Windows)
    drop(s.files);

    match ClipboardContext::new() {
        Ok(ctx) => {
            match ctx.set_files(paths.clone()) {
                Ok(()) => info!("[{}] Clipboard files set ({} items)", peer_id, paths.len()),
                Err(e) => {
                    warn!("[{}] set_files failed: {} — falling back to URI text", peer_id, e);
                    // Fallback: set as plain URI text
                    let uri_text = paths.iter()
                        .map(|p| format!("file://{p}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Err(e2) = ctx.set_text(uri_text) {
                        error!("[{}] set_text fallback error: {}", peer_id, e2);
                    }
                }
            }
        }
        Err(e) => error!("[{}] clipboard context error: {}", peer_id, e),
    }
}

/// Set the system clipboard to plain text via clipboard-rs.
fn set_clipboard_text(text: &str, peer_id: &str) {
    match ClipboardContext::new() {
        Ok(ctx) => match ctx.set_text(text.to_owned()) {
            Ok(()) => info!("[{}] Clipboard text set ({} bytes)", peer_id, text.len()),
            Err(e) => error!("[{}] set_text error: {}", peer_id, e),
        },
        Err(e) => error!("[{}] clipboard context error: {}", peer_id, e),
    }
}

/// Set the system clipboard to an RGBA image via clipboard-rs.
fn set_clipboard_image(rgba: &[u8], width: u32, height: u32, peer_id: &str) {
    let rgba_img = match image::RgbaImage::from_raw(width, height, rgba.to_vec()) {
        Some(img) => img,
        None => {
            error!("[{}] failed to create RgbaImage from raw data", peer_id);
            return;
        }
    };
    let img = RustImageData::from_dynamic_image(
        image::DynamicImage::ImageRgba8(rgba_img),
    );
    match ClipboardContext::new() {
        Ok(ctx) => match ctx.set_image(img) {
            Ok(()) => info!("[{}] Clipboard image set ({}x{})", peer_id, width, height),
            Err(e) => error!("[{}] set_image error: {}", peer_id, e),
        },
        Err(e) => error!("[{}] clipboard context error: {}", peer_id, e),
    }
}

/// Periodically monitor the host's system clipboard and push changes to the viewer.
///
/// Uses hash-based loop prevention: when content is received from the viewer and written
/// to the local clipboard, we record its hash so the writer can skip echoing it back.
///
/// Monitors: text, file-list, and image clipboards.
pub fn spawn_clipboard_writer(dc: Arc<dyn DataChannel>, running: Arc<AtomicBool>, peer_id: String) {
    tokio::spawn(async move {
        info!("[{}] Clipboard writer started (200ms poll)", peer_id);
        let mut last_text = String::new();
        let mut last_file_uris = String::new();
        let mut last_image_hash: u64 = 0;
        while running.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(frame::CLIPBOARD_WRITE_POLL_MS)).await;

            let ctx = match ClipboardContext::new() {
                Ok(c) => c,
                Err(e) => {
                    warn!("[{}] clipboard context error: {}", peer_id, e);
                    continue;
                }
            };

            // ── Check text clipboard ─────────────────
            if let Ok(text) = ctx.get_text() {
                if !text.is_empty() && text != last_text {
                    let hash = text_hash(&text);
                    let should_send = match CLIP_SYNC.lock() {
                        Ok(sync) => hash != sync.last_received_hash,
                        Err(_) => true,
                    };
                    if should_send {
                        let mut payload = vec![frame::CLIP_TAG_TEXT];
                        payload.extend_from_slice(text.as_bytes());
                        if dc.send(BytesMut::from(&payload[..])).await.is_ok() {
                            info!("[{}] Sent clipboard text ({} bytes)", peer_id, text.len());
                        }
                    }
                    last_text = text;
                }
            }

            // ── Check file URI clipboard ─────────────
            if let Some(uris) = get_clipboard_file_uris(&ctx) {
                if !uris.is_empty() && uris != last_file_uris {
                    send_host_clipboard_files(&dc, &uris, &peer_id, &mut last_file_uris).await;
                }
            }

            // ── Check image clipboard ────────────────
            if let Ok(img) = ctx.get_image() {
                let rgba = match img.to_rgba8() {
                    Ok(rgba_img) => rgba_img.into_raw(),
                    Err(_) => continue,
                };
                if !rgba.is_empty() {
                    let h = bytes_hash(&rgba);
                    let should_send = {
                        let echo = match CLIP_IMAGE_SYNC.lock() {
                            Ok(sync) => h == sync.last_received_hash,
                            Err(_) => false,
                        };
                        h != last_image_hash && !echo
                    };
                    if should_send {
                        let (width, height) = img.get_size();
                        if rgba.len() <= frame::CLIP_IMAGE_MAX_BYTES {
                            let mut payload = Vec::with_capacity(1 + 8 + rgba.len());
                            payload.push(frame::CLIP_TAG_IMAGE);
                            payload.extend_from_slice(&(width as u32).to_le_bytes());
                            payload.extend_from_slice(&(height as u32).to_le_bytes());
                            payload.extend_from_slice(&rgba);
                            if dc.send(BytesMut::from(&payload[..])).await.is_ok() {
                                info!("[{}] Sent clipboard image: {}x{} ({} bytes)",
                                      peer_id, width, height, rgba.len());
                            }
                        } else {
                            warn!("[{}] Clipboard image too large ({} bytes), skipping",
                                  peer_id, rgba.len());
                        }
                        last_image_hash = h;
                    }
                }
            }
        }
        info!("[{}] Clipboard writer ended", peer_id);
    });
}

/// Read file paths from the system clipboard via clipboard-rs.
/// Returns a newline-separated `file://` URI string, or `None`.
fn get_clipboard_file_uris(ctx: &ClipboardContext) -> Option<String> {
    let files = ctx.get_files().ok()?;
    if files.is_empty() {
        return None;
    }
    let uris: Vec<String> = files.into_iter().map(|p| {
        if p.starts_with("file://") { p } else { format!("file://{p}") }
    }).collect();
    Some(uris.join("\n"))
}

/// Package and send files from the host clipboard to the viewer.
async fn send_host_clipboard_files(
    dc: &Arc<dyn DataChannel>,
    uris: &str,
    peer_id: &str,
    last_file_uris: &mut String,
) {
    let chunk_size = frame::FILE_CHUNK_SIZE;

    // Parse URIs → local paths
    let paths: Vec<PathBuf> = uris
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("file://") {
                Some(PathBuf::from(&line[7..]))
            } else if !line.is_empty() {
                Some(PathBuf::from(line))
            } else {
                None
            }
        })
        .collect();
    if paths.is_empty() {
        return;
    }

    // Build file list header: [0x03][count:u16le] + for each: [name_len:u16le][name:utf8][size:u64le]
    let mut list_buf = Vec::with_capacity(frame::CLIP_FILE_LIST_BUF);
    list_buf.push(frame::CLIP_TAG_FILE_LIST);
    list_buf.extend_from_slice(&(paths.len() as u16).to_le_bytes());
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

    if dc.send(BytesMut::from(&list_buf[..])).await.is_err() {
        warn!("[{}] Failed to send host file list", peer_id);
        return;
    }

    // Send chunks per file
    for (i, p) in paths.iter().enumerate() {
        let data = match std::fs::read(p) {
            Ok(d) => d,
            Err(e) => {
                warn!("[{}] Cannot read {}: {}", peer_id, p.display(), e);
                continue;
            }
        };
        let mut offset = 0u64;
        while offset < data.len() as u64 {
            let len = std::cmp::min(chunk_size, data.len() - offset as usize);
            let mut chunk = Vec::with_capacity(frame::CLIP_CHUNK_MSG_SIZE + len);
            chunk.push(frame::CLIP_TAG_FILE_CHUNK);
            chunk.extend_from_slice(&(i as u16).to_le_bytes());
            chunk.extend_from_slice(&offset.to_le_bytes());
            chunk.extend_from_slice(&(len as u32).to_le_bytes());
            chunk.extend_from_slice(&data[offset as usize..offset as usize + len]);
            if dc.send(BytesMut::from(&chunk[..])).await.is_err() {
                warn!("[{}] Failed to send host file chunk", peer_id);
                break;
            }
            offset += len as u64;
        }
    }

    // Complete marker
    if let Err(e) = dc.send(BytesMut::from(&[frame::CLIP_TAG_DONE][..])).await {
        error!("[{}] Failed to send clipboard done marker: {}", peer_id, e);
    }
    info!("[{}] Sent {} host clipboard files to viewer", peer_id, paths.len());
    *last_file_uris = uris.to_string();
}
