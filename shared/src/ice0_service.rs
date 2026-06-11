//! ice0-based RPC service definition for RTCDesk.
//!
//! This module defines the RPC interface between components using ice0:
//! - Client ↔ Host-Service (via IPC channel)
//! - Host-Service ↔ Signal-Server (via TCP)
//!
//! Uses ice0's `#[ice_service]` macro to generate type-safe service definitions
//! with automatic serialization/deserialization.

use serde::{Deserialize, Serialize};
use ice0::prelude::*;

// ─── Data Structures ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResult {
    pub success: bool,
    pub token: String,
    pub user_id: i64,
    pub username: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResult {
    pub success: bool,
    pub user_id: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineInfo {
    pub node_id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
    pub is_bound: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidateInfo {
    pub from: String,
    pub to: String,
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub accepted: bool,
    pub reason: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatusChange {
    pub node_id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerConnectionRequest {
    pub viewer_id: String,
    pub host_id: String,
    pub password: String,
    pub offer_sdp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub connected: bool,
    pub host_id: String,
}

// ─── Service Definitions ─────────────────────────────────────────────────────

/// Host Service RPC interface.
/// 
/// This trait defines all the operations that can be invoked
/// via RPC. The same trait is used for:
/// - Client → Host-Service communication (IPC transport)
/// - Host-Service → Signal-Server communication (TCP transport)
#[ice_service]
pub trait SignalService {
    /// Authenticate user with signal server.
    async fn user_login(username: String, password: String) -> LoginResult;
    
    /// Logout current user from signal server.
    async fn user_logout();
    
    /// Register a new user with signal server.
    async fn user_register(username: String, password: String, email: String) -> RegisterResult;
    
    /// Request the list of online machines for the authenticated user.
    async fn machine_list(token: String) -> Vec<MachineInfo>;
        
    /// Bind a machine to current user (requires authentication token).
    async fn machine_bind(token: String, host_id: String, name: String, os: String);
        
    /// Register a machine (online notification).
    async fn machine_online(host_id: String, name: String, os: String);
    
    
    /// Unbind a machine from current user (requires authentication token).
    async fn machine_unbind(token: String, host_id: String);
    
    /// Handle viewer connection request with Offer SDP.
    /// 
    /// This is the ONLY function needed for WebRTC connection setup.
    /// The viewer creates an Offer SDP and sends it to the host.
    /// The host validates the password, creates an Answer SDP, and returns it.
    async fn viewer_connect_with_offer(
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
    ) -> Answer;
    
    /// Send viewer's ICE candidate to host (oneway - fire and forget).
    async fn ice_candidate(
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    );

    /// Update streaming quality for a viewer.
    async fn viewer_quality_update(
        viewer_id: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32,
    );
}

// ─── Callback Services (Server → Client push notifications) ──────────────────

/// Host Callback Service: Signal Server → Host-Service notifications.
/// 
/// This allows the signal server to push notifications to host-service:
/// - Peer status changes (online/offline)
/// - Viewer connection requests (with Answer response)
/// - ICE candidates from viewers
#[ice_service]
pub trait HostCallback {
    /// Notify host about peer status change (online/offline).
    async fn on_peer_status_change(change: PeerStatusChange);
    
    /// Notify host about incoming viewer connection request.
    /// 
    /// This is a SYNCHRONOUS callback - the host MUST return an Answer.
    async fn request_control_from_viewer(request: ViewerConnectionRequest) -> Answer;
    
    /// Forward ICE candidate from remote host to viewer (oneway - fire and forget).
    /// 
    /// This allows the signal server to relay ICE candidates
    /// from remote host to viewer for WebRTC connection establishment.
    async fn on_ice_candidate(info: IceCandidateInfo);
}

/// Client Callback Service: Host-Service → Client notifications.
/// 
/// This allows host-service to push notifications to client:
/// - Machine status changes
/// - Streaming events
/// - ICE candidates from remote host (for WebRTC connection setup)
#[ice_service]
pub trait ClientCallback {
    /// Notify client about peer/machine status change.
    async fn on_peer_status_change(change: PeerStatusChange);
    
    /// Notify client about streaming quality change.
    async fn on_streaming_event(viewer_id: String, event: String, data: String);
    
    /// Forward ICE candidate from remote host to client (oneway - fire and forget).
    async fn on_ice_candidate(info: IceCandidateInfo);
}
