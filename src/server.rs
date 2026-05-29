//! WebSocket signaling server for RTCDesk.
//!
//! Handles peer registration, room management, and message forwarding
//! between hosts and viewers for WebRTC session establishment.

use crate::frame;
use crate::signal::{PeerRole, SignalMessage};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Helper: log an error when a channel `send()` fails.
macro_rules! log_send_err {
    ($expr:expr, $ctx:expr) => {
        if let Err(e) = $expr {
            warn!("[Server] {}: send failed: {}", $ctx, e);
        }
    };
}
/// Helper: log an error when a broadcast `send()` fails.
macro_rules! log_broadcast_err {
    ($expr:expr, $ctx:expr) => {
        if let Err(e) = $expr {
            warn!("[Server] {}: broadcast failed: {}", $ctx, e);
        }
    };
}

/// Shared application state for the signaling server.
pub struct SignalingState {
    /// Connected peers: id -> (role, message_tx)
    peers: RwLock<HashMap<String, PeerInfo>>,
    /// Broadcast channel for peer join/leave events.
    room_events: broadcast::Sender<RoomEvent>,
}

#[derive(Debug, Clone)]
struct PeerInfo {
    role: PeerRole,
    sender: mpsc::UnboundedSender<String>,
}

/// Room-level events broadcast to all connected peers.
#[derive(Debug, Clone)]
pub enum RoomEvent {
    PeerJoined { id: String, role: PeerRole },
    PeerLeft { id: String },
}

impl Default for SignalingState {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(frame::SIGNAL_BROADCAST_CAP);
        Self {
            peers: RwLock::new(HashMap::new()),
            room_events: tx,
        }
    }
}

impl SignalingState {
    /// Create a new signaling state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a new peer.
    async fn register(&self, peer_id: &str, role: PeerRole, sender: mpsc::UnboundedSender<String>) {
        let mut peers = self.peers.write().await;

        // Remove existing peer with same ID if any
        if peers.remove(peer_id).is_some() {
            info!("Removing existing peer with ID: {}", peer_id);
            log_broadcast_err!(
                self.room_events.send(RoomEvent::PeerLeft {
                    id: peer_id.to_string(),
                }),
                "PeerLeft remove-existing"
            );
        }

        let info = PeerInfo {
            role: role.clone(),
            sender,
        };
        peers.insert(peer_id.to_string(), info);

        info!("Registered new peer: {} as {:?}", peer_id, role);
        log_broadcast_err!(
            self.room_events.send(RoomEvent::PeerJoined {
                id: peer_id.to_string(),
                role,
            }),
            "PeerJoined"
        );
    }

    /// Unregister a peer.
    async fn unregister(&self, peer_id: &str) {
        let mut peers = self.peers.write().await;
        if peers.remove(peer_id).is_some() {
            info!("Unregistering peer: {}", peer_id);
            log_broadcast_err!(
                self.room_events.send(RoomEvent::PeerLeft {
                    id: peer_id.to_string(),
                }),
                "PeerLeft unregister"
            );
        }
    }

    /// Forward a signaling message to a specific peer.
    async fn forward_message(&self, to: &str, message: &str) -> bool {
        let peers = self.peers.read().await;
        if let Some(peer) = peers.get(to) {
            info!("Forwarding message to peer {}: {:?}", to, message);
            match peer.sender.send(message.to_string()) {
                Ok(()) => {
                    info!("Message successfully sent to {}", to);
                    true
                }
                Err(e) => {
                    warn!("Failed to send message to {}: {}", to, e);
                    false
                }
            }
        } else {
            warn!("Target peer not found: {}", to);
            false
        }
    }

    /// List all viewers currently connected.
    async fn get_viewers(&self) -> Vec<String> {
        let peers = self.peers.read().await;
        peers
            .iter()
            .filter(|(_, info)| info.role == PeerRole::Viewer)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// List all hosts currently connected.
    async fn get_hosts(&self) -> Vec<String> {
        let peers = self.peers.read().await;
        peers
            .iter()
            .filter(|(_, info)| info.role == PeerRole::Host)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Start the signaling server on the given address.
pub async fn start_signaling_server(
    state: Arc<SignalingState>,
    bind_addr: &str,
) -> anyhow::Result<()> {
    let state_clone = state.clone();

    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| handle_ws_upgrade(ws, state_clone.clone())),
    );

    info!("Signaling server listening on {}", bind_addr);

    info!("Binding TCP listener to {}", bind_addr);
    let listener = tokio::net::TcpListener::bind(bind_addr).await
        .map_err(|e| {
            error!("Failed to bind TCP listener: {}", e);
            e
        })?;
    
    info!("Starting axum server");
    axum::serve(listener, app).await
        .map_err(|e| {
            error!("Axum server error: {}", e);
            e
        })?;

    Ok(())
}

/// Handle a WebSocket upgrade request.
async fn handle_ws_upgrade(ws: WebSocketUpgrade, state: Arc<SignalingState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
}

/// Handle an individual WebSocket connection lifecycle.
async fn handle_ws_connection(ws: WebSocket, state: Arc<SignalingState>) {
    let (mut ws_sender, mut ws_receiver) = ws.split();

    // Channel for sending messages to this connection
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let mut peer_id = String::new();
    let mut peer_role: Option<PeerRole> = None;

    // Subscribe to room events
    let mut room_rx = state.room_events.subscribe();

    info!("New WebSocket connection established");

    // Task 1: Forward internal channel messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sender.send(Message::Text(msg.into())).await {
                error!("WebSocket send error: {}", e);
                break;
            }
        }
    });

    // Task 2: Read WebSocket messages and handle them
    loop {
        tokio::select! {
            // Incoming WebSocket message from this peer
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        info!("Received message from client: {}", text);
                        if let Err(e) = handle_signaling_message(
                            &text,
                            &mut peer_id,
                            &mut peer_role,
                            &state,
                            &tx,
                        ).await {
                            error!(
                                "Error handling message from {}: {}",
                                if peer_id.is_empty() { "unknown" } else { &peer_id },
                                e
                            );
                            let err_json = serde_json::to_string(&SignalMessage::Error {
                                message: format!("Server error: {}", e),
                            }).unwrap_or_else(|se| {
                                error!("[Server] Failed to serialize error message: {}", se);
                                r#"{"type":"error","message":"internal error"}"#.to_string()
                            });
                            log_send_err!(tx.send(err_json), "error response");
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Client {} disconnected", peer_id);
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        info!("Received ping from client, sending pong");
                        if let Err(e) = tx.send(String::from_utf8_lossy(&data).to_string()) {
                            error!("Failed to send pong response: {}", e);
                        }
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        warn!("WebSocket channel closed unexpectedly");
                        break;
                    }
                    _ => {
                        warn!("Received unexpected message type");
                    }
                }
            }

            // Room-level events (peer join/leave)
            roomevt = room_rx.recv() => {
                match roomevt {
                    Ok(event) => {
                        info!("Received room event: {:?}", event);
                        if let Err(e) = handle_room_event(event, &peer_id, &tx).await {
                            error!("Error handling room event: {}", e);
                        }
                    }
                    // Handle channel errors
                    Err(e) => {
                        error!("Error receiving room events: {}", e);
                    }
                }
            }

        }
    }

    // Cleanup on disconnect
    if !peer_id.is_empty() {
        info!("Peer {} left", peer_id);
        state.unregister(&peer_id).await;
    } else {
        warn!("Connection closed without peer registration");
    }

    send_task.abort();
    info!("WebSocket connection closed");
}

/// Process a single signaling message from a peer.
async fn handle_signaling_message(
    text: &str,
    peer_id: &mut String,
    peer_role: &mut Option<PeerRole>,
    state: &Arc<SignalingState>,
    tx: &mpsc::UnboundedSender<String>,
) -> anyhow::Result<()> {
    info!("Handling signaling message: {}", text);
    
    let msg: SignalMessage = serde_json::from_str(text)
        .map_err(|e| {
            error!("Failed to parse signal message: {}", e);
            e
        })?;
    
    log::debug!("Received: {:?}", msg);

    match &msg {
        SignalMessage::Register { id, role } => {
            info!("Processing register message for peer: {} as {:?}", id, role);
            handle_register(id, role, peer_id, peer_role, state, tx).await
                .map_err(|e| {
                    error!("Registration error: {}", e);
                    e
                })?;
        }

        SignalMessage::Offer {
            from: _,
            to,
            sdp: _,
        }
        | SignalMessage::Answer {
            from: _,
            to,
            sdp: _,
        }
        | SignalMessage::IceCandidate { from: _, to, .. }
        | SignalMessage::QualityUpdate { from: _, to, .. } => {
            info!("Server forwarding to '{}'", to);
            if !state.forward_message(to, text).await {
                warn!("Failed to forward message to '{}' (peer not found)", to);
                let err = SignalMessage::Error {
                    message: format!("Target peer '{}' not found", to),
                };
                log_send_err!(tx.send(serde_json::to_string(&err)?), "error on forward");
            }
        }
        SignalMessage::Input {
            from,
            to,
            event_type: _,
            data: _,
        } => {
            info!("Server received input event from {} to {}", from, to);
            if !state.forward_message(to, text).await {
                warn!("Failed to forward input event to {} (peer not found)", to);
                let err = SignalMessage::Error {
                    message: format!("Target peer '{}' not found", to),
                };
                log_send_err!(tx.send(serde_json::to_string(&err)?), "error on input forward");
            }
        }

        SignalMessage::RequestConnect { target_id, .. } => {
            info!("Processing request connect message to target: {}", target_id);
            let enriched = SignalMessage::RequestConnect {
                from: peer_id.clone(),
                target_id: target_id.clone(),
            };
            let enriched_text = serde_json::to_string(&enriched)
                .map_err(|e| {
                    error!("Failed to serialize enriched connect message: {}", e);
                    e
                })?;
            
            if !state.forward_message(target_id, &enriched_text).await {
                warn!("Failed to forward connect request to {} (peer not found)", target_id);
                let err = SignalMessage::Error {
                    message: format!("Target peer '{}' not found", target_id),
                };
                log_send_err!(tx.send(serde_json::to_string(&err)?), "error on connect forward");
            }
        }

        SignalMessage::Error { message } => {
            warn!("Received error message: {}", message);
            // Error messages are just logged and not forwarded
        }
        
        _ => {
            warn!("Received unsupported message type: {:?}", msg);
        }
    }

    Ok(())
}

/// Handle a Register message: store peer info and notify about existing peers.
async fn handle_register(
    id: &str,
    role: &PeerRole,
    peer_id: &mut String,
    peer_role: &mut Option<PeerRole>,
    state: &Arc<SignalingState>,
    tx: &mpsc::UnboundedSender<String>,
) -> anyhow::Result<()> {
    *peer_id = id.to_string();
    *peer_role = Some(role.clone());
    state.register(id, role.clone(), tx.clone()).await;

    let ack = SignalMessage::Registered {
        assigned_id: id.to_string(),
    };
    log_send_err!(tx.send(serde_json::to_string(&ack)?), "register ack");

    // Notify about existing peers in the room
    let hosts = state.get_hosts().await;
    for host_id in &hosts {
        if host_id != id {
            let notify = SignalMessage::PeerJoined {
                id: host_id.clone(),
                role: PeerRole::Host,
            };
            log_send_err!(tx.send(serde_json::to_string(&notify)?), "existing host notify");
        }
    }
    let viewers = state.get_viewers().await;
    for viewer_id in &viewers {
        if viewer_id != id {
            let notify = SignalMessage::PeerJoined {
                id: viewer_id.clone(),
                role: PeerRole::Viewer,
            };
            log_send_err!(tx.send(serde_json::to_string(&notify)?), "existing viewer notify");
        }
    }

    info!("Peer registered: {} as {:?}", id, role);
    Ok(())
}

/// Handle a room-level event (peer join/leave).
async fn handle_room_event(
    event: RoomEvent,
    my_id: &str,
    tx: &mpsc::UnboundedSender<String>,
) -> anyhow::Result<()> {
    match event {
        RoomEvent::PeerJoined { id, role } => {
            if id != my_id {
                let msg = SignalMessage::PeerJoined { id, role };
                log_send_err!(tx.send(serde_json::to_string(&msg)?), "room PeerJoined");
            }
        }
        RoomEvent::PeerLeft { id } => {
            if id != my_id {
                let msg = SignalMessage::PeerLeft { id };
                log_send_err!(tx.send(serde_json::to_string(&msg)?), "room PeerLeft");
            }
        }
    }
    Ok(())
}

/// Helper for debug logging in async context.
fn debug(msg: &str) {
    log::debug!("{}", msg);
}
