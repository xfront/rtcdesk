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
    /// Connect request with video quality parameter and optional password.
    ConnectRequest {
        from: String,
        target_id: String,
        pic_quality: u32,
        /// Password for host authentication (empty = no password).
        #[serde(default)]
        password: String,
    },
    /// Host authentication result (accepted / rejected).
    AuthResult {
        from: String,
        to: String,
        accepted: bool,
        /// Rejection reason (only meaningful when accepted=false).
        #[serde(default)]
        reason: String,
    },
    /// Viewer requests a quality, resolution, or FPS update during an active session.
    QualityUpdate {
        from: String,
        to: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
    },
    /// 输入事件（鼠标/键盘）
    Input {
        from: String,
        to: String,
        event_type: InputEventType,
        data: Vec<u8>,
    },

    // ─── 用户认证相关 ────────────────────────────────────────────────────

    /// 用户登录请求
    Login {
        username: String,
        password: String,
    },

    /// 登录结果
    LoginResult {
        success: bool,
        #[serde(default)]
        token: String,
        #[serde(default)]
        reason: String,
    },

    /// Token 刷新请求
    TokenRefresh {
        token: String,
    },

    /// Token 刷新结果
    TokenRefreshResult {
        success: bool,
        #[serde(default)]
        token: String,
        #[serde(default)]
        reason: String,
    },

    // ─── 机器管理相关 ────────────────────────────────────────────────────

    /// Host 注册机器信息
    MachineRegister {
        host_id: String,
        name: String,
        os: String,
    },

    /// 在线机器列表
    MachineList {
        machines: Vec<MachineInfo>,
    },

    /// 机器在线状态变更通知
    MachineStatus {
        host_id: String,
        online: bool,
    },

    /// 心跳消息（Host 定期发送）
    Heartbeat {
        host_id: String,
    },

    // ─── 用户注册 ────────────────────────────────────────────────────

    /// 用户注册请求
    UserRegister {
        username: String,
        password: String,
    },

    /// 用户注册结果
    UserRegisterResult {
        success: bool,
        #[serde(default)]
        message: String,
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

/// 在线机器信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineInfo {
    /// 机器唯一标识（即 host peer ID）
    pub host_id: String,
    /// 机器名称
    pub name: String,
    /// 操作系统
    pub os: String,
    /// 是否在线
    pub online: bool,
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
