//! WebSocket 信令处理器。
//!
//! 处理来自 Host/Viewer 客户端的 WebSocket 连接，
//! 包括：登录认证、peer 注册、机器注册、心跳、消息转发。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};

use shared::signal::{PeerRole, SignalMessage};

use crate::auth::AuthService;
use crate::machine::MachineRegistry;

// ─── 连接状态 ──────────────────────────────────────────────────────────

/// 单个已连接 peer 的状态。
struct PeerState {
    /// peer 的 ID（来自 Register 消息）。
    peer_id: String,
    /// peer 角色（Host / Viewer）。
    role: PeerRole,
    /// 关联的用户 ID（来自登录认证）。
    user_id: i64,
    /// 用户名。
    username: String,
    /// WebSocket 发送端。
    sender: tokio::sync::mpsc::UnboundedSender<Message>,
}

/// 所有已连接 peer 的共享注册表。
pub struct PeerRegistry {
    peers: tokio::sync::Mutex<HashMap<String, PeerState>>,
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self {
            peers: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn insert(&self, peer_id: String, state: PeerState) {
        self.peers.lock().await.insert(peer_id, state);
    }

    async fn remove(&self, peer_id: &str) {
        self.peers.lock().await.remove(peer_id);
    }

    async fn get_sender(&self, peer_id: &str) -> Option<tokio::sync::mpsc::UnboundedSender<Message>> {
        self.peers.lock().await.get(peer_id).map(|p| p.sender.clone())
    }

    async fn get_role(&self, peer_id: &str) -> Option<PeerRole> {
        self.peers.lock().await.get(peer_id).map(|p| p.role.clone())
    }

    async fn get_user_id(&self, peer_id: &str) -> Option<i64> {
        self.peers.lock().await.get(peer_id).map(|p| p.user_id)
    }
}

/// 共享的应用状态。
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub machines: Arc<MachineRegistry>,
    pub peers: Arc<PeerRegistry>,
}

// ─── WebSocket 升级入口 ────────────────────────────────────────────────

/// axum WebSocket 升级处理器。
///
/// 接受 WebSocket 连接并进入消息循环。
pub async fn ws_upgrade(
    axum::extract::State(state): axum::extract::State<AppState>,
    ws: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle_ws(state, socket))
}

// ─── WebSocket 消息循环 ────────────────────────────────────────────────

/// 处理单个 WebSocket 连接的生命周期。
async fn handle_ws(state: AppState, socket: WebSocket) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 为转发 fan-out 创建 mpsc channel
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

    // 后台任务：将 mpsc channel 中的消息写入 WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // ─── 阶段 1：登录认证 ────────────────────────────────────────────

    let (user_id, username, _token) = loop {
        let Some(Ok(Message::Text(text))) = ws_receiver.next().await else {
            return;
        };

        let msg: SignalMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid JSON from client: {}", e);
                continue;
            }
        };

        match msg {
            SignalMessage::Login {
                username,
                password,
            } => {
                match state.auth.login(&username, &password) {
                    Ok(token) => {
                        let claims = state
                            .auth
                            .verify_token(&token)
                            .expect("just-issued token should be valid");
                        let result = SignalMessage::LoginResult {
                            success: true,
                            token: token.clone(),
                            reason: String::new(),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                        info!("User '{}' authenticated via WS", username);
                        break (claims.sub.parse::<i64>().unwrap_or(0), claims.username, token);
                    }
                    Err(e) => {
                        let result = SignalMessage::LoginResult {
                            success: false,
                            token: String::new(),
                            reason: format!("{}", e),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                        warn!("WS login failed for '{}': {}", username, e);
                        continue;
                    }
                }
            }
            SignalMessage::UserRegister {
                username: ref reg_user,
                password: ref reg_pass,
            } => {
                match state.auth.register(reg_user, reg_pass) {
                    Ok((_uid, token)) => {
                        let result = SignalMessage::UserRegisterResult {
                            success: true,
                            message: format!("User '{}' registered successfully", reg_user),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                        // 注册成功后直接使用返回的 token 完成登录
                        let claims = state
                            .auth
                            .verify_token(&token)
                            .expect("just-issued token should be valid");
                        info!("User '{}' registered via WS", reg_user);
                        break (claims.sub.parse::<i64>().unwrap_or(0), claims.username, token);
                    }
                    Err(e) => {
                        let result = SignalMessage::UserRegisterResult {
                            success: false,
                            message: format!("{}", e),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                        warn!("WS user registration failed for '{}': {}", reg_user, e);
                        continue;
                    }
                }
            }
            SignalMessage::TokenRefresh { token: ref tok } => {
                match state.auth.refresh_token(tok) {
                    Ok(new_token) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: true,
                            token: new_token,
                            reason: String::new(),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                    Err(e) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: false,
                            token: String::new(),
                            reason: format!("{}", e),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                }
                continue;
            }
            _ => {
                let err = SignalMessage::Error {
                    message: "Must Login first".to_string(),
                };
                let json = serde_json::to_string(&err).unwrap();
                let _ = tx.send(Message::Text(json.into()));
                continue;
            }
        }
    };

    // ─── 阶段 2：peer 注册 ────────────────────────────────────────────

    let (peer_id, peer_role) = loop {
        let Some(Ok(Message::Text(text))) = ws_receiver.next().await else {
            return;
        };

        let msg: SignalMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid JSON: {}", e);
                continue;
            }
        };

        match msg {
            SignalMessage::Register { id, role } => {
                let result = SignalMessage::Registered {
                    assigned_id: id.clone(),
                };
                let json = serde_json::to_string(&result).unwrap();
                let _ = tx.send(Message::Text(json.into()));

                state
                    .peers
                    .insert(
                        id.clone(),
                        PeerState {
                            peer_id: id.clone(),
                            role: role.clone(),
                            user_id,
                            username: username.clone(),
                            sender: tx.clone(),
                        },
                    )
                    .await;

                info!(
                    "Peer '{}' registered as {:?} (user='{}')",
                    id, role, username
                );
                break (id, role);
            }
            SignalMessage::TokenRefresh { token: ref tok } => {
                match state.auth.refresh_token(tok) {
                    Ok(new_token) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: true,
                            token: new_token,
                            reason: String::new(),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                    Err(e) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: false,
                            token: String::new(),
                            reason: format!("{}", e),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                }
            }
            _ => {
                let err = SignalMessage::Error {
                    message: "Must Register after Login".to_string(),
                };
                let json = serde_json::to_string(&err).unwrap();
                let _ = tx.send(Message::Text(json.into()));
            }
        }
    };

    // ─── 阶段 3：消息主循环 ──────────────────────────────────────────

    while let Some(msg_result) = ws_receiver.next().await {
        let text = match msg_result {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) => {
                info!("Peer '{}' disconnected", peer_id);
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = tx.send(Message::Pong(data));
                continue;
            }
            Err(e) => {
                warn!("WS error for peer '{}': {}", peer_id, e);
                break;
            }
            _ => continue,
        };

        let msg: SignalMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid JSON from '{}': {}", peer_id, e);
                continue;
            }
        };

        match msg {
            // ── 机器注册 ──────────────────────────────────────────
            SignalMessage::MachineRegister {
                host_id,
                name,
                os,
            } => {
                if let Err(e) = state
                    .machines
                    .register(&host_id, user_id, &name, &os)
                {
                    error!("MachineRegister error: {}", e);
                }
            }

            // ── 心跳 ──────────────────────────────────────────────
            SignalMessage::Heartbeat { host_id } => {
                if let Err(e) = state.machines.heartbeat(&host_id) {
                    error!("Heartbeat error for '{}': {}", host_id, e);
                }
            }

            // ── Token 刷新 ────────────────────────────────────────
            SignalMessage::TokenRefresh { token: ref tok } => {
                match state.auth.refresh_token(tok) {
                    Ok(new_token) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: true,
                            token: new_token,
                            reason: String::new(),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                    Err(e) => {
                        let result = SignalMessage::TokenRefreshResult {
                            success: false,
                            token: String::new(),
                            reason: format!("{}", e),
                        };
                        let json = serde_json::to_string(&result).unwrap();
                        let _ = tx.send(Message::Text(json.into()));
                    }
                }
            }

            // ── 消息转发 ──────────────────────────────────────────
            // 以下消息类型需要转发给目标 peer：
            SignalMessage::RequestConnect {
                ref from,
                ref target_id,
            }
            | SignalMessage::ConnectRequest {
                ref from,
                ref target_id,
                pic_quality: _,
                password: _,
            }
            | SignalMessage::Offer {
                ref from,
                to: ref target_id,
                ..
            }
            | SignalMessage::Answer {
                ref from,
                to: ref target_id,
                ..
            }
            | SignalMessage::IceCandidate {
                ref from,
                to: ref target_id,
                ..
            }
            | SignalMessage::AuthResult {
                ref from,
                to: ref target_id,
                ..
            }
            | SignalMessage::QualityUpdate {
                ref from,
                to: ref target_id,
                ..
            }
            | SignalMessage::Input {
                ref from,
                to: ref target_id,
                ..
            } => {
                let json = serde_json::to_string(&msg).unwrap();
                if let Some(sender) = state.peers.get_sender(target_id).await {
                    if sender.send(Message::Text(json.into())).is_err() {
                        warn!("Failed to forward message to '{}'", target_id);
                    }
                } else {
                    // 目标离线，通知来源
                    warn!(
                        "Target peer '{}' not found for message from '{}'",
                        target_id, from
                    );
                    let err = SignalMessage::Error {
                        message: format!("Peer '{}' is not online", target_id),
                    };
                    let err_json = serde_json::to_string(&err).unwrap();
                    let _ = tx.send(Message::Text(err_json.into()));
                }
            }

            // ── 在线机器列表查询 ─────────────────────────────────
            SignalMessage::MachineList { .. } => {
                let machines = state.machines.get_online_machines(user_id);
                let reply = SignalMessage::MachineList { machines };
                let json = serde_json::to_string(&reply).unwrap();
                let _ = tx.send(Message::Text(json.into()));
            }

            // ── 以下消息类型不需要服务器处理，忽略 ────────────────
            SignalMessage::Login { .. }
            | SignalMessage::LoginResult { .. }
            | SignalMessage::Register { .. }
            | SignalMessage::Registered { .. }
            | SignalMessage::PeerJoined { .. }
            | SignalMessage::PeerLeft { .. }
            | SignalMessage::MachineStatus { .. }
            | SignalMessage::TokenRefreshResult { .. }
            | SignalMessage::UserRegister { .. }
            | SignalMessage::UserRegisterResult { .. }
            | SignalMessage::Error { .. } => {
                warn!("Unexpected message type from '{}': ignored", peer_id);
            }
        }
    }

    // ── 断开连接，清理 peer 注册 ───────────────────────────────────────

    info!("Peer '{}' ({:?}) disconnected", peer_id, peer_role);
    state.peers.remove(&peer_id).await;

    // 通知其他 peer 该 peer 已离开（可选）
    // 当前设计：客户端自己通过 WebSocket 断开来感知

    // 关闭发送任务
    send_task.abort();
}
