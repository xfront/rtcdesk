//! RTCDesk Host Service — Remote Desktop Control via WebRTC.
//!
//! Entry point for the standalone host service.
//! Connects to a remote signaling server, authenticates, and
//! handles WebRTC-based desktop sharing.
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p host-service
//! ```
//!
//! Environment variables (override config file values):
//! - `RTCDESK_ADDR` — signaling server address (default: 127.0.0.1:9009)
//! - `RTCDESK_ID` — host peer ID (default: auto-generated UUID)
//! - `RTCDESK_FPS` — streaming frame rate (default: 15)
//! - `RTCDESK_STUN` — STUN server URLs, comma-separated (default: empty / LAN-only)
//! - `RTCDESK_PASSWORD` — access password for viewers (default: empty / open access)
//! - `RTCDESK_USERNAME` — login username for signaling server (default: admin)
//! - `RTCDESK_LOGIN_PASSWORD` — login password for signaling server (default: admin123)

mod capture;
mod peer;
mod signal_client;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;
use webrtc::peer_connection::RTCSdpType;

use shared::{frame, local_id, signal};
use shared::signal::SignalMessage;

use crate::capture::ScreenCapturer;
use crate::peer::RtcPeer;
use crate::signal_client::{SignalClient, SharedWs, WsRead};

// ── Defaults ────────────────────────────────────────────────────────────────

const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:9009";
const DEFAULT_STUN: &str = "";
const DEFAULT_FPS: u32 = frame::DEFAULT_FPS;
const DEFAULT_PASSWORD: &str = "";
const DEFAULT_USERNAME: &str = "admin";
const DEFAULT_LOGIN_PASSWORD: &str = "admin123";
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

// ── Config file ─────────────────────────────────────────────────────────────

/// Persistent host configuration stored in `~/.config/rtcdesk/host.toml`
/// (or `%APPDATA%\rtcdesk\host.toml` on Windows).
#[derive(Debug, Serialize, Deserialize)]
struct HostConfig {
    #[serde(default)]
    host: HostConfigSection,
}

#[derive(Debug, Serialize, Deserialize)]
struct HostConfigSection {
    #[serde(default)]
    host_id: String,
    #[serde(default = "default_server_addr")]
    server_addr: String,
    #[serde(default = "default_username")]
    username: String,
    #[serde(default = "default_login_password")]
    password: String,
    #[serde(default)]
    stun: String,
}

fn default_server_addr() -> String {
    DEFAULT_SERVER_ADDR.to_string()
}
fn default_username() -> String {
    DEFAULT_USERNAME.to_string()
}
fn default_login_password() -> String {
    DEFAULT_LOGIN_PASSWORD.to_string()
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            host: HostConfigSection::default(),
        }
    }
}

impl Default for HostConfigSection {
    fn default() -> Self {
        Self {
            host_id: String::new(),
            server_addr: default_server_addr(),
            username: default_username(),
            password: default_login_password(),
            stun: String::new(),
        }
    }
}

/// Get the config file path using the `dirs` crate for cross-platform support.
fn config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rtcdesk")
        .join("host.toml")
}

/// Load config from disk, creating a default one if it doesn't exist.
fn load_config() -> Result<HostConfig> {
    let path = config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let config: HostConfig = toml::from_str(&content)
            .with_context(|| format!("failed to parse config file: {}", path.display()))?;
        Ok(config)
    } else {
        Ok(HostConfig::default())
    }
}

/// Save config to disk, creating parent directories as needed.
fn save_config(config: &HostConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir: {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(config)
        .context("failed to serialize config")?;
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write config file: {}", path.display()))?;
    Ok(())
}

/// Ensure the config has a host_id; generate and persist one if missing.
/// Uses the same algorithm as the client (`shared::local_id`) for consistency.
fn ensure_host_id(config: &mut HostConfig) -> &str {
    if config.host.host_id.is_empty() {
        config.host.host_id = local_id::generate_local_id();
        info!("Generated new host_id: {}", config.host.host_id);
        if let Err(e) = save_config(config) {
            warn!("Failed to persist host_id to config: {}", e);
        }
    }
    &config.host.host_id
}

// ── Host context ────────────────────────────────────────────────────────────

/// Holds the initialized host state.
struct HostContext {
    host_id: String,
    client: SignalClient,
    ws_read: WsRead,
    capturer: Option<Arc<ScreenCapturer>>,
    stun: Vec<String>,
    sessions: Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
    /// Access password (empty = no auth required).
    password: String,
    fps: u32,
}

/// Initialize the host: connect to signaling server, login, and set up capture.
async fn init_host() -> Result<HostContext> {
    // Install rustls crypto provider
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    info!("=== RTCDesk Host Service ===");

    // Load persistent config
    let mut config = load_config()?;
    let host_id = ensure_host_id(&mut config).to_string();
    info!("Host ID: {}", host_id);

    // Resolve settings: env vars override config file
    let server_addr = std::env::var("RTCDESK_ADDR")
        .unwrap_or_else(|_| config.host.server_addr.clone());
    let username = std::env::var("RTCDESK_USERNAME")
        .unwrap_or_else(|_| config.host.username.clone());
    let login_password = std::env::var("RTCDESK_LOGIN_PASSWORD")
        .unwrap_or_else(|_| config.host.password.clone());

    // Connect and send Login
    let (client, ws_read) = SignalClient::connect(
        &server_addr,
        &host_id,
        &username,
        &login_password,
    )
    .await?;

    // Initialize screen capture
    let capturer = ScreenCapturer::new()
        .map(|c| {
            let (w, h) = c.resolution();
            info!("Screen capturer: {}x{}", w, h);
            c.set_max_dimension(frame::DEFAULT_CAPTURE_DIM);
            Arc::new(c)
        })
        .map_err(|e| warn!("Screen capturer init failed: {}", e))
        .ok();

    let stun: Vec<String> = std::env::var("RTCDESK_STUN")
        .unwrap_or_else(|_| config.host.stun.clone())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if stun.is_empty() {
        info!("No STUN configured — LAN-only mode (set RTCDESK_STUN for NAT traversal)");
    } else {
        info!("STUN servers: {:?}", stun);
    }

    let password = std::env::var("RTCDESK_PASSWORD")
        .unwrap_or_else(|_| DEFAULT_PASSWORD.to_string());
    if password.is_empty() {
        info!("No password configured — open access mode");
    } else {
        info!("Password authentication enabled");
    }

    let fps: u32 = std::env::var("RTCDESK_FPS")
        .unwrap_or_else(|_| DEFAULT_FPS.to_string())
        .parse()
        .unwrap_or(DEFAULT_FPS);

    Ok(HostContext {
        host_id,
        client,
        ws_read,
        capturer,
        stun,
        sessions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        password,
        fps,
    })
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = init_host().await?;
    let HostContext {
        host_id,
        client,
        mut ws_read,
        capturer,
        stun,
        sessions,
        password,
        fps,
    } = ctx;

    info!("Waiting for LoginResult from signaling server...");

    let run = run_host_loop(
        &host_id,
        &client,
        &mut ws_read,
        &stun,
        capturer.as_ref(),
        fps,
        &sessions,
        &password,
    );

    tokio::select! {
        res = run => {
            if let Err(ref e) = res {
                error!("Host loop exited with error: {}", e);
            }
            res
        }
        _ = shutdown_signal() => {
            info!("Received shutdown signal, exiting gracefully...");
            Ok(())
        }
    }
}

// ── Shutdown signal ──────────────────────────────────────────────────────────

/// Wait for a shutdown signal (SIGTERM, SIGINT, or Ctrl+C).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        if let Ok(mut sigterm) = sigterm {
            let sigint = tokio::signal::ctrl_c();
            tokio::select! {
                _ = sigterm.recv() => {
                    info!("SIGTERM received");
                }
                _ = sigint => {
                    info!("SIGINT received");
                }
            }
            return;
        }
    }
    // Fallback / Windows: just wait for Ctrl+C
    let _ = tokio::signal::ctrl_c().await;
    info!("Ctrl+C received");
}

// ── Host loop ────────────────────────────────────────────────────────────────

/// Tracks whether the host has completed the login + registration flow.
struct RegistrationState {
    /// Whether we have received a successful LoginResult.
    logged_in: bool,
    /// Whether we have received Registered confirmation.
    registered: bool,
    /// Handle for the heartbeat task (started after registration).
    heartbeat_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Main message processing loop for the host.
///
/// Handles the login/registration flow first, then processes
/// normal WebRTC signaling messages.
async fn run_host_loop(
    host_id: &str,
    client: &SignalClient,
    ws_read: &mut WsRead,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
    password: &str,
) -> Result<()> {
    let mut reg_state = RegistrationState {
        logged_in: false,
        registered: false,
        heartbeat_handle: None,
    };

    // Get OS info for MachineRegister
    let machine_name = match std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("NAME"))
    {
        Ok(name) => name,
        Err(_) => "unknown".to_string(),
    };
    let machine_os = std::env::consts::OS.to_string();

    while let Some(msg) = ws_read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let signal_msg: SignalMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Bad signaling message: {}", e);
                        continue;
                    }
                };
                info!("Host recv: {:?}", signal_msg);

                // Handle login/registration flow messages first
                if !reg_state.logged_in {
                    match &signal_msg {
                        SignalMessage::LoginResult { success, token, reason } => {
                            if *success {
                                info!("Login successful, token received");
                                reg_state.logged_in = true;
                                // Store token in client
                                // Note: We can't mutate client.token directly here since
                                // client is borrowed. Token is stored for future use.
                                let _token = token.clone();

                                // Send Register + MachineRegister
                                client.send_register().await?;
                                client.send_machine_register(&machine_name, &machine_os).await?;
                            } else {
                                error!("Login failed: {}", reason);
                                info!("Please ensure the user exists in the signaling server database.");
                                info!("You can create a user with: RTCDESK_USERNAME=xxx RTCDESK_LOGIN_PASSWORD=xxx");
                                return Ok(());
                            }
                            continue;
                        }
                        _ => {
                            // Before login, ignore other messages
                            warn!("Received message before login: {:?}", signal_msg);
                            continue;
                        }
                    }
                }

                if !reg_state.registered {
                    match &signal_msg {
                        SignalMessage::Registered { assigned_id } => {
                            info!("Registered as host, assigned_id={}", assigned_id);
                            reg_state.registered = true;

                            // Start heartbeat
                            reg_state.heartbeat_handle =
                                Some(client.start_heartbeat(HEARTBEAT_INTERVAL_SECS));

                            info!("Host '{}' is online and ready for viewers", host_id);
                        }
                        SignalMessage::Error { message } => {
                            error!("Server error during registration: {}", message);
                            anyhow::bail!("Registration error: {}", message);
                        }
                        _ => {
                            // Before registration complete, defer other messages
                            // (but still process LoginResult if it comes late)
                        }
                    }
                    continue;
                }

                // Normal message dispatch after registration
                if let Err(e) = dispatch_host_message(
                    signal_msg,
                    host_id,
                    &client.ws_write,
                    stun,
                    capturer,
                    fps,
                    sessions,
                    password,
                )
                .await
                {
                    error!("Failed to handle signaling message from {}: {}", host_id, e);
                }
            }
            Ok(Message::Close(_)) => {
                info!("Signaling closed");
                break;
            }
            Err(e) => {
                error!("WS error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Cleanup: stop heartbeat first (so we don't try to send over closed WS)
    if let Some(h) = reg_state.heartbeat_handle {
        h.abort();
    }

    // Gracefully close all active WebRTC sessions before shutdown.
    // Without this, spawned tasks holding PeerConnection/DataChannel
    // references keep running, and tokio runtime shutdown can trigger
    // segfaults in webrtc-rs internal I/O threads.
    {
        let mut sessions = sessions.lock().await;
        let count = sessions.len();
        if count > 0 {
            info!(
                "Closing {} active session(s) before host shutdown...",
                count
            );
            for (viewer_id, peer) in sessions.iter() {
                let timeout = tokio::time::Duration::from_secs(3);
                info!("Closing session with viewer '{}'", viewer_id);
                match tokio::time::timeout(timeout, peer.close()).await {
                    Ok(Ok(())) => info!("Session '{}' closed gracefully", viewer_id),
                    Ok(Err(e)) => error!("Error closing session '{}': {}", viewer_id, e),
                    Err(_) => warn!("Timed out closing session '{}'", viewer_id),
                }
            }
            sessions.clear();
        }
    }

    // Allow a short grace period for spawned tasks (input/clipboard
    // readers, stream loop, ICE forwarder) to detect running==false and exit.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    info!("RTCDesk Host shutting down");
    Ok(())
}

// ── Message dispatch ─────────────────────────────────────────────────────────

/// Route a single signaling message to the appropriate handler.
async fn dispatch_host_message(
    msg: SignalMessage,
    host_id: &str,
    ws: &Arc<SharedWs>,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
    password: &str,
) -> Result<()> {
    match msg {
        SignalMessage::LoginResult { success, token, reason } => {
            if success {
                info!("Late LoginResult received (token length={}), ignoring", token.len());
            } else {
                error!("Late LoginResult failure: {}", reason);
            }
        }
        SignalMessage::MachineStatus { host_id: mid, online } => {
            info!("Machine {} status: online={}", mid, online);
        }
        SignalMessage::TokenRefreshResult { success, token, reason } => {
            if success {
                info!("Token refresh successful (new token length={})", token.len());
            } else {
                warn!("Token refresh failed: {}", reason);
            }
        }
        SignalMessage::RequestConnect { from, target_id } if target_id == *host_id => {
            info!("Viewer '{}' connection requested", from);
            // Legacy path: no password field — reject if host requires password
            if !password.is_empty() {
                info!("[Auth] Rejecting legacy RequestConnect from {} (no password)", from);
                send_auth_result(ws, host_id, &from, false, "密码认证失败：此主机需要密码").await?;
                return Ok(());
            }
            handle_connect(host_id, &from, ws, stun, capturer, fps, sessions).await?;
        }
        SignalMessage::ConnectRequest { from, target_id, pic_quality, password: ref pwd } if target_id == *host_id => {
            info!("Viewer '{}' ConnectRequest (quality={})", from, pic_quality);
            // Password verification
            if !password.is_empty() {
                if pwd != password {
                    info!("[Auth] Wrong password from {}", from);
                    send_auth_result(ws, host_id, &from, false, "密码错误").await?;
                    return Ok(());
                }
                info!("[Auth] Password accepted for {}", from);
                send_auth_result(ws, host_id, &from, true, "").await?;
            }
            handle_connect(host_id, &from, ws, stun, capturer, fps, sessions).await?;
        }
        SignalMessage::Offer { from, to, sdp } if to == *host_id => {
            info!("Offer from {}", from);
            handle_remote_offer(host_id, &from, &sdp, ws, stun, capturer, fps, sessions).await?;
        }
        SignalMessage::Answer { from, sdp, .. } => {
            info!("Answer from {}", from);
            if let Some((_, peer)) = sessions.lock().await.iter().find(|(id, _)| id == &from) {
                // Apply viewer-requested capturer params (quality, resolution).
                peer.apply_capturer_params_from_sdp(&sdp);
                if let Err(e) = peer.set_remote_description(&sdp, RTCSdpType::Answer).await {
                    error!("[Host] set_remote_description(Answer) from {}: {}", from, e);
                }

                // Now start streaming with the viewer's preferred FPS
                let cp = signal::extract_capturer_params_from_sdp(&sdp);
                let viewer_fps = cp.fps.unwrap_or(fps);
                info!(
                    "[{}] Starting stream for {} at {} fps",
                    host_id, from, viewer_fps
                );
                let p = peer.clone();
                tokio::spawn(async move {
                    if let Err(e) = p.start_streaming("desktop", viewer_fps).await {
                        error!("Streaming error: {}", e);
                    }
                });
            }
        }
        SignalMessage::IceCandidate {
            from,
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            if let Some((_, peer)) = sessions.lock().await.iter().find(|(id, _)| id == &from) {
                if let Err(e) = peer
                    .add_ice_candidate(
                        &candidate,
                        &sdp_mid.unwrap_or_default(),
                        sdp_mline_index.unwrap_or(0),
                    )
                    .await
                {
                    error!("[Host] add_ice_candidate from {}: {}", from, e);
                }
            }
        }
        SignalMessage::QualityUpdate {
            from,
            quality,
            max_dimension,
            fps,
            ..
        } => {
            info!(
                "[{}] QualityUpdate from {}: quality={} dim={} fps={}",
                host_id, from, quality, max_dimension, fps
            );
            if let Some((_, peer)) = sessions.lock().await.iter().find(|(id, _)| id == &from) {
                peer.update_quality(quality, max_dimension, fps);
            }
        }
        SignalMessage::PeerJoined { id, role } => {
            info!("Peer joined: {} ({:?})", id, role);
        }
        SignalMessage::PeerLeft { id } => {
            info!("Peer left: {}", id);
            // Gracefully close the peer connection before removing from sessions.
            if let Some((_, peer)) = sessions.lock().await.iter().find(|(sid, _)| sid == &id) {
                let timeout = tokio::time::Duration::from_secs(3);
                let p = peer.clone();
                match tokio::time::timeout(timeout, p.close()).await {
                    Ok(Ok(())) => info!("Closed peer connection for '{}'", id),
                    Ok(Err(e)) => error!("Error closing peer '{}': {}", id, e),
                    Err(_) => warn!("Timed out closing peer '{}'", id),
                }
            }
            sessions.lock().await.retain(|(sid, _)| sid != &id);
        }
        _ => {}
    }
    Ok(())
}

// ── ICE forwarder ────────────────────────────────────────────────────────────

/// Spawn a task that forwards ICE candidates from the peer to the viewer
/// via the shared WebSocket signaling channel.
fn spawn_ice_forwarder(
    peer: &mut RtcPeer,
    ws: &Arc<SharedWs>,
    viewer_id: &str,
) -> Result<()> {
    let mut ice_rx = peer
        .take_ice_receiver()
        .context("ICE receiver already taken")?;
    let ice_ws = ws.clone();
    let ice_viewer_id = viewer_id.to_string();
    tokio::spawn(async move {
        while let Some(mut msg) = ice_rx.recv().await {
            if let SignalMessage::IceCandidate { to, .. } = &mut msg {
                *to = ice_viewer_id.clone();
            }
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("[ICE fwd] serialize error: {}", e);
                    continue;
                }
            };
            let mut w = ice_ws.lock().await;
            if let Err(e) = w.send(Message::Text(json.into())).await {
                error!("[ICE fwd] WS send error: {}", e);
            }
        }
    });
    Ok(())
}

// ── Input & clipboard setup ──────────────────────────────────────────────────

/// Create the "input" and "clipboard" data channels for a viewer session
/// and spawn the corresponding reader/writer tasks.
async fn setup_input_and_clipboard(
    peer: &Arc<RtcPeer>,
    viewer_id: &str,
) -> Result<()> {
    let input_dc = peer.create_frame_channel("input").await?;
    let running = peer.running_flag();
    let input_peer_id = viewer_id.to_string();
    peer::spawn_input_reader(input_dc, running.clone(), input_peer_id);

    let clipboard_dc = peer.create_frame_channel("clipboard").await?;
    let clip_peer_id = viewer_id.to_string();
    let clip_dc_writer = clipboard_dc.clone();
    peer::spawn_clipboard_reader(clipboard_dc, running.clone(), clip_peer_id.clone());
    peer::spawn_clipboard_writer(clip_dc_writer, running, clip_peer_id);

    Ok(())
}

// ── Connection handling ──────────────────────────────────────────────────────

async fn handle_connect(
    host_id: &str,
    viewer_id: &str,
    mut ws: &Arc<SharedWs>,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    _fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
) -> Result<()> {
    // Remove any stale session for this viewer (e.g. from a previous
    // connection whose WebSocket didn't close cleanly). This prevents
    // duplicate sessions and ensures Answer/IceCandidate dispatch
    // matches the correct peer.
    // Close the old peer in a background task to avoid blocking the
    // new connection setup (pc.close() can take several seconds).
    {
        let mut sessions = sessions.lock().await;
        if let Some(idx) = sessions.iter().position(|(id, _)| id == viewer_id) {
            let old_peer = sessions[idx].1.clone();
            info!("Cleaning up stale session for viewer '{}'", viewer_id);
            sessions.remove(idx);
            drop(sessions);
            tokio::spawn(async move {
                info!("Closing stale peer connection for viewer '{}'", old_peer.peer_id());
                if let Err(e) = old_peer.close().await {
                    warn!("Error closing stale peer: {}", e);
                }
                info!("Stale peer for viewer '{}' closed", old_peer.peer_id());
            });
        }
    }

    let mut peer = RtcPeer::new_host(host_id, stun).await?;

    spawn_ice_forwarder(&mut peer, ws, viewer_id)?;

    if let Some(c) = capturer {
        peer.attach_capturer(c.clone())?;
    }

    let dc = peer.create_frame_channel("desktop").await?;
    peer.set_frame_channel(dc);

    let peer = Arc::new(peer);
    setup_input_and_clipboard(&peer, viewer_id).await?;

    let offer = peer.create_offer().await?;

    let msg = SignalMessage::Offer {
        from: host_id.to_string(),
        to: viewer_id.to_string(),
        sdp: offer.sdp,
    };
    ws.lock()
        .await
        .send(Message::Text(serde_json::to_string(&msg)?.into()))
        .await
        .map_err(|_e| anyhow::anyhow!("WS send error"))?;
    info!("Sent offer to {}", viewer_id);

    sessions.lock().await.push((viewer_id.to_string(), peer));
    Ok(())
}

/// Send an AuthResult signaling message to a viewer.
async fn send_auth_result(
    ws: &Arc<SharedWs>,
    host_id: &str,
    viewer_id: &str,
    accepted: bool,
    reason: &str,
) -> Result<()> {
    let msg = SignalMessage::AuthResult {
        from: host_id.to_string(),
        to: viewer_id.to_string(),
        accepted,
        reason: reason.to_string(),
    };
    ws.lock()
        .await
        .send(Message::Text(serde_json::to_string(&msg)?.into()))
        .await
        .map_err(|_e| anyhow::anyhow!("WS send error"))?;
    if accepted {
        info!("[Auth] Sent AuthResult accepted to {}", viewer_id);
    } else {
        info!("[Auth] Sent AuthResult rejected to {}: {}", viewer_id, reason);
    }
    Ok(())
}

async fn handle_remote_offer(
    host_id: &str,
    viewer_id: &str,
    sdp_str: &str,
    ws: &Arc<SharedWs>,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    _fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
) -> Result<()> {
    // Remove stale session for this viewer if any
    {
        let mut sessions = sessions.lock().await;
        if let Some(idx) = sessions.iter().position(|(id, _)| id == viewer_id) {
            let old_peer = sessions[idx].1.clone();
            info!("Cleaning up stale session for viewer '{}'", viewer_id);
            sessions.remove(idx);
            drop(sessions);
            tokio::spawn(async move {
                if let Err(e) = old_peer.close().await {
                    warn!("Error closing stale peer: {}", e);
                }
            });
        }
    }

    let mut peer = RtcPeer::new_host(host_id, stun).await?;

    if let Some(c) = capturer {
        peer.attach_capturer(c.clone())?;
    }

    peer.apply_capturer_params_from_sdp(sdp_str);
    info!(
        "[{}] Applied capturer params from remote Offer SDP",
        host_id
    );

    spawn_ice_forwarder(&mut peer, ws, viewer_id)?;

    let peer = Arc::new(peer);

    peer.set_remote_description(sdp_str, RTCSdpType::Offer)
        .await?;

    setup_input_and_clipboard(&peer, viewer_id).await?;

    let answer = peer.create_answer().await?;

    let msg = SignalMessage::Answer {
        from: host_id.to_string(),
        to: viewer_id.to_string(),
        sdp: answer.sdp,
    };
    ws.lock()
        .await
        .send(Message::Text(serde_json::to_string(&msg)?.into()))
        .await
        .map_err(|_e| anyhow::anyhow!("WS send error"))?;
    info!("Sent answer to {}", viewer_id);

    let cp = signal::extract_capturer_params_from_sdp(sdp_str);
    let viewer_fps = cp.fps.unwrap_or(_fps);
    info!(
        "[{}] Starting stream for {} at {} fps",
        host_id, viewer_id, viewer_fps
    );
    let p = peer.clone();
    tokio::spawn(async move {
        if let Err(e) = p.start_streaming("desktop", viewer_fps).await {
            error!("Streaming error: {}", e);
        }
    });

    sessions.lock().await.push((viewer_id.to_string(), peer));
    Ok(())
}
