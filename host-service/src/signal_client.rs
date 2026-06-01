//! WebSocket signaling client for host-service.
//!
//! Encapsulates the WS connection to a remote signaling server,
//! including login authentication, peer registration, machine registration,
//! heartbeat, and message send/receive.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use std::sync::Arc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use shared::signal::{PeerRole, SignalMessage};

/// Shared WebSocket writer type — mutex-wrapped so it can be shared across tasks
/// (especially the ICE-candidate forward task and heartbeat timer).
pub type SharedWs = tokio::sync::Mutex<
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
>;

/// WS read stream type — consumed by the main message loop.
pub type WsRead = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
>;

/// Manages the signaling connection lifecycle for the host.
///
/// The typical startup flow is:
/// 1. `SignalClient::connect()` — establish WS, send `Login`
/// 2. Main loop receives `LoginResult` — store token
/// 3. Main loop sends `Register` + `MachineRegister`
/// 4. Main loop receives `Registered` — registration complete
/// 5. `SignalClient::start_heartbeat()` — begin periodic heartbeat
pub struct SignalClient {
    /// Shared WS write half for sending signaling messages.
    pub ws_write: Arc<SharedWs>,
    /// Host peer ID (persistent, doubles as machine ID).
    pub host_id: String,
    /// JWT token received from LoginResult.
    pub token: String,
}

impl SignalClient {
    /// Connect to the remote signaling server and send a Login request.
    ///
    /// Returns `(SignalClient, WsRead)` where `WsRead` should be consumed
    /// in the main message loop. The `LoginResult` will arrive on that
    /// read stream and must be processed there (token extracted, then
    /// Register + MachineRegister sent).
    pub async fn connect(
        server_addr: &str,
        host_id: &str,
        username: &str,
        password: &str,
    ) -> Result<(Self, WsRead)> {
        let ws_url = format!("ws://{}/ws", server_addr);
        info!("Connecting to signaling server at {}", ws_url);

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .context("failed to connect to signaling server")?;
        let (ws_write, ws_read) = ws_stream.split();
        let ws_write = Arc::new(SharedWs::new(ws_write));

        // Step 1: Send Login
        let login_msg = SignalMessage::Login {
            username: username.to_string(),
            password: password.to_string(),
        };
        send_message(&ws_write, &login_msg).await?;
        info!("Login request sent for user '{}', waiting for LoginResult", username);

        Ok((
            Self {
                ws_write,
                host_id: host_id.to_string(),
                token: String::new(), // Filled when LoginResult is received
            },
            ws_read,
        ))
    }

    /// Send a signaling message over the WebSocket.
    pub async fn send(&self, msg: &SignalMessage) -> Result<()> {
        send_message(&self.ws_write, msg).await
    }

    /// Send the Register message (host role) after successful login.
    pub async fn send_register(&self) -> Result<()> {
        let msg = SignalMessage::Register {
            id: self.host_id.clone(),
            role: PeerRole::Host,
        };
        send_message(&self.ws_write, &msg).await?;
        info!("Register request sent as host '{}'", self.host_id);
        Ok(())
    }

    /// Send the MachineRegister message after successful login.
    pub async fn send_machine_register(&self, machine_name: &str, machine_os: &str) -> Result<()> {
        let msg = SignalMessage::MachineRegister {
            host_id: self.host_id.clone(),
            name: machine_name.to_string(),
            os: machine_os.to_string(),
        };
        send_message(&self.ws_write, &msg).await?;
        info!(
            "MachineRegister sent: id={}, name={}, os={}",
            self.host_id, machine_name, machine_os
        );
        Ok(())
    }

    /// Start a periodic heartbeat timer that sends `Heartbeat` messages.
    ///
    /// Returns a `JoinHandle` that can be aborted to stop heartbeats.
    pub fn start_heartbeat(&self, interval_secs: u64) -> tokio::task::JoinHandle<()> {
        let ws = self.ws_write.clone();
        let host_id = self.host_id.clone();
        info!("Starting heartbeat timer (interval={}s)", interval_secs);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                let msg = SignalMessage::Heartbeat {
                    host_id: host_id.clone(),
                };
                if let Err(e) = send_message(&ws, &msg).await {
                    error!("Heartbeat send error: {}", e);
                    break;
                }
            }
        })
    }
}

/// Send a signaling message as JSON text over the shared WebSocket writer.
pub async fn send_message(ws: &Arc<SharedWs>, msg: &SignalMessage) -> Result<()> {
    let json = serde_json::to_string(msg)?;
    ws.lock()
        .await
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| anyhow::anyhow!("WS send error: {}", e))
}
