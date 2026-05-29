//! Signaling protocol message types for RTCDesk.
//!
//! Defines the JSON-based signaling protocol used between
//! the WebSocket server and clients (both host & viewer).

use serde::{Deserialize, Serialize};

/// Top-level signaling message exchanged over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    /// Viewer/Host registers with the signaling server.
    Register { id: String, role: PeerRole },
    /// Server acknowledges registration.
    Registered { assigned_id: String },
    /// A new peer has joined the room.
    PeerJoined { id: String, role: PeerRole },
    /// A peer has left the room.
    PeerLeft { id: String },
    /// SDP Offer from one peer to another.
    Offer {
        from: String,
        to: String,
        sdp: String,
    },
    /// SDP Answer from one peer to another.
    Answer {
        from: String,
        to: String,
        sdp: String,
    },
    /// ICE candidate from one peer to another.
    IceCandidate {
        from: String,
        to: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    /// Viewer requests to connect to a specific host.
    RequestConnect { from: String, target_id: String },
    /// Error message.
    Error { message: String },
    /// Connect request with video quality parameter.
    ConnectRequest {
        from: String,
        target_id: String,
        pic_quality: u32,
    },
    /// Viewer requests a quality, resolution, or FPS update during an active session.
    QualityUpdate {
        from: String,
        to: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
    },
    /// Input event (mouse/keyboard).
    Input {
        from: String,
        to: String,
        event_type: InputEventType,
        data: Vec<u8>,
    },
}

/// Input event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEventType {
    MouseMove,
    MouseDown,
    MouseUp,
    KeyDown,
    KeyUp,
}

/// Role of a peer in the remote desktop session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    /// Desktop sharing host (Rust server).
    Host,
    /// Remote viewer/controller (KMP client).
    Viewer,
}

/// ICE server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServerConfig {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

// ─── Capturer params via SDP ─────────────────────────────────────────

/// Host capturer configuration received from the viewer via custom SDP attributes.
#[derive(Debug, Clone, Default)]
pub struct CapturerParams {
    /// JPEG quality (0-100).
    pub pic_quality: Option<u32>,
    /// Max output dimension; frames larger than this are scaled down.
    pub max_dimension: Option<u32>,
    /// Target frame rate.
    pub fps: Option<u32>,
}

/// Encode capturer params as SDP session-level attribute lines.
/// Appends `\n`-separated `a=rtcdesk-*` attributes.
pub fn encode_capturer_params_to_sdp(params: &CapturerParams) -> String {
    let mut out = String::new();
    if let Some(q) = params.pic_quality {
        out.push_str(&format!("a=rtcdesk-quality:{}\n", q));
    }
    if let Some(d) = params.max_dimension {
        out.push_str(&format!("a=rtcdesk-max-dim:{}\n", d));
    }
    if let Some(f) = params.fps {
        out.push_str(&format!("a=rtcdesk-fps:{}\n", f));
    }
    out
}

/// Extract `CapturerParams` embedded in SDP attribute lines.
pub fn extract_capturer_params_from_sdp(sdp: &str) -> CapturerParams {
    let mut params = CapturerParams::default();
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("a=rtcdesk-quality:") {
            params.pic_quality = val.parse().ok();
        } else if let Some(val) = line.strip_prefix("a=rtcdesk-max-dim:") {
            params.max_dimension = val.parse().ok();
        } else if let Some(val) = line.strip_prefix("a=rtcdesk-fps:") {
            params.fps = val.parse().ok();
        }
    }
    params
}
