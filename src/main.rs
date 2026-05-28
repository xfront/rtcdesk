//! RTCDesk - Remote Desktop Control via WebRTC.
//!
//! Entry point for the Rust host application.
//! Starts the signaling server and handles WebRTC-based desktop sharing.
//!
//! ## Usage
//!
//! ```bash
//! cargo run
//! ```
//!
//! Environment variables:
//! - `RTCDESK_ADDR` - signaling server bind address (default: 0.0.0.0:9009)
//! - `RTCDESK_ID` - host peer ID (default: auto-generated UUID)
//! - `RTCDESK_FPS` - streaming frame rate (default: 15)

mod capture;
mod frame;
mod peer;
mod server;
mod signal;

use crate::capture::ScreenCapturer;
use crate::peer::RtcPeer;
use crate::server::SignalingState;
use crate::signal::{PeerRole, SignalMessage};
use anyhow::{Context, Result};
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use std::sync::Arc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
use webrtc::peer_connection::RTCSdpType;

/// Shared WebSocket writer type — mutex-wrapped so it can be shared across tasks
/// (especially the ICE-candidate forward task).
type SharedWs = tokio::sync::Mutex<
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
>;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:9009";
/// Comma-separated STUN server URLs (e.g. "stun:your-server:3478,stun:backup:3478").
/// Leave empty for LAN-only (no NAT traversal). Set via `RTCDESK_STUN` env var.
const DEFAULT_STUN: &str = "";
const LOCAL_DEFAULT_FPS: u32 = frame::DEFAULT_FPS;

/// Holds the initialized host state.
struct HostContext {
    host_id: String,
    ws_write: Arc<SharedWs>,
    ws_read: SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    capturer: Option<Arc<ScreenCapturer>>,
    stun: Vec<String>,
    sessions: Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
}

async fn init_host() -> Result<HostContext> {
    let bind_addr = std::env::var("RTCDESK_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());
    let host_id =
        std::env::var("RTCDESK_ID").unwrap_or_else(|_| format!("host-{}", Uuid::new_v4()));

    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    info!("=== RTCDesk Host ===");

    // Start signaling server in background
    let state = SignalingState::new();
    let state_clone = state.clone();
    let addr_clone = bind_addr.clone();
    tokio::spawn(async move {
        if let Err(e) = server::start_signaling_server(state_clone, &addr_clone).await {
            error!("Signaling server error: {}", e);
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(frame::SIGNAL_STARTUP_DELAY_MS)).await;

    // Connect to own signaling server as host.
    // 0.0.0.0 is only valid for listening, not connecting — use 127.0.0.1.
    let connect_addr = bind_addr.replacen("0.0.0.0", "127.0.0.1", 1);
    let ws_url = format!("ws://{}/ws", connect_addr);
    info!("Connecting as host '{}' to {}", host_id, ws_url);
    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .context("failed to connect to signaling server")?;
    let (ws_write, ws_read) = ws_stream.split();
    let ws_write = Arc::new(SharedWs::new(ws_write));

    // Register as host
    let register = SignalMessage::Register {
        id: host_id.clone(),
        role: PeerRole::Host,
    };
    ws_write
        .lock()
        .await
        .send(Message::Text(serde_json::to_string(&register)?.into()))
        .await
        .map_err(|_e| anyhow::anyhow!("register send error"))?;
    info!("Registered as host '{}'", host_id);

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
        .unwrap_or_else(|_| DEFAULT_STUN.to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if stun.is_empty() {
        info!("No STUN configured — LAN-only mode (set RTCDESK_STUN for NAT traversal)");
    } else {
        info!("STUN servers: {:?}", stun);
    }

    Ok(HostContext {
        host_id,
        ws_write,
        ws_read,
        capturer,
        stun,
        sessions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let fps: u32 = std::env::var("RTCDESK_FPS")
        .unwrap_or_else(|_| LOCAL_DEFAULT_FPS.to_string())
        .parse()
        .unwrap_or(LOCAL_DEFAULT_FPS);

    let ctx = init_host().await?;
    let HostContext {
        host_id,
        ws_write,
        mut ws_read,
        capturer,
        stun,
        sessions,
    } = ctx;

    info!("Waiting for viewers...");
    run_host_loop(
        &host_id,
        &ws_write,
        &mut ws_read,
        &stun,
        capturer.as_ref(),
        fps,
        &sessions,
    )
    .await
}

/// Main message processing loop for the host.
async fn run_host_loop(
    host_id: &str,
    ws_write: &Arc<SharedWs>,
    ws_read: &mut (
             impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin
         ),
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
) -> Result<()> {
    while let Some(msg) = ws_read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let msg: SignalMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Bad signaling message: {}", e);
                        continue;
                    }
                };
                info!("Host recv: {:?}", msg);
                if let Err(e) = dispatch_host_message(msg, host_id, ws_write, stun, capturer, fps, sessions)
                    .await
                {
                    error!("Failed to handle signaling message from {}: {}", host_id, e);
                    // Don't crash the whole server because of one failed message
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
    info!("RTCDesk Host shutting down");
    Ok(())
}

/// Route a single signaling message to the appropriate handler.
async fn dispatch_host_message(
    msg: SignalMessage,
    host_id: &str,
    ws: &Arc<SharedWs>,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
) -> Result<()> {
    match msg {
        SignalMessage::RequestConnect { from, target_id } if target_id == *host_id => {
            info!("Viewer '{}' connection requested", from);
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
                let _ = peer.set_remote_description(&sdp, RTCSdpType::Answer).await;

                // Now start streaming with the viewer's preferred FPS
                // instead of the hard-coded default.
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
                let _ = peer
                    .add_ice_candidate(
                        &candidate,
                        &sdp_mid.unwrap_or_default(),
                        sdp_mline_index.unwrap_or(0),
                    )
                    .await;
            }
        }
        SignalMessage::PeerJoined { id, role } => {
            info!("Peer joined: {} ({:?})", id, role);
        }
        SignalMessage::PeerLeft { id } => {
            info!("Peer left: {}", id);
            sessions.lock().await.retain(|(sid, _)| sid != &id);
        }
        _ => {}
    }
    Ok(())
}

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
            let json = serde_json::to_string(&msg).unwrap_or_default();
            let mut w = ice_ws.lock().await;
            let _ = w.send(Message::Text(json.into())).await;
        }
    });
    Ok(())
}

/// Create the "input" and "clipboard" data channels for a viewer session
/// and spawn the corresponding reader/writer tasks.
async fn setup_input_and_clipboard(
    peer: &Arc<RtcPeer>,
    viewer_id: &str,
) -> Result<()> {
    let input_dc = peer.create_frame_channel("input").await?;
    let running = peer.running_flag();
    let input_peer_id = viewer_id.to_string();
    crate::peer::spawn_input_reader(input_dc, running.clone(), input_peer_id);

    let clipboard_dc = peer.create_frame_channel("clipboard").await?;
    let clip_peer_id = viewer_id.to_string();
    let clip_dc_writer = clipboard_dc.clone();
    crate::peer::spawn_clipboard_reader(clipboard_dc, running.clone(), clip_peer_id.clone());
    crate::peer::spawn_clipboard_writer(clip_dc_writer, running, clip_peer_id);
    Ok(())
}

async fn handle_connect(
    host_id: &str,
    viewer_id: &str,
    ws: &Arc<SharedWs>,
    stun: &[String],
    capturer: Option<&Arc<ScreenCapturer>>,
    _fps: u32,
    sessions: &Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
) -> Result<()> {
    let mut peer = RtcPeer::new_host(host_id, stun).await?;

    spawn_ice_forwarder(&mut peer, ws, viewer_id)?;

    if let Some(c) = capturer {
        peer.attach_capturer(c.clone())?;
    }

    // Create data channel BEFORE creating the offer so that the
    // offer SDP includes the data channel media section with ICE credentials.
    let dc = peer.create_frame_channel("desktop").await?;
    peer.set_frame_channel(dc);

    let peer = Arc::new(peer);
    setup_input_and_clipboard(&peer, viewer_id).await?;

    // Create offer
    let offer = peer.create_offer().await?;

    // Send offer to viewer
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

    // Don't start streaming here — wait for the Answer which carries
    // the viewer's preferred fps, quality, and resolution. Streaming
    // will be started when the Answer is received (see dispatch_host_message).
    sessions.lock().await.push((viewer_id.to_string(), peer));
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
    let mut peer = RtcPeer::new_host(host_id, stun).await?;

    if let Some(c) = capturer {
        peer.attach_capturer(c.clone())?;
    }

    // Extract capturer params from the viewer's Offer SDP
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

    // Start streaming with the viewer's preferred FPS from the Offer SDP
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
