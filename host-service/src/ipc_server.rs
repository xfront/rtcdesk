//! IPC server using ice0 with interprocess sockets.
//!
//! This uses ice0 RPC framework over interprocess sockets (Unix) or named pipes (Windows)
//! for communication between the client GUI and host-service.

use crate::peer::RtcPeer;
use crate::signal_client::SignalClient;
use ice0::prelude::*;
use log::{error, info, warn};
use shared::ice0_service::{
    Answer, ConnectionStatus, IceCandidateInfo, LoginResult, MachineInfo, PeerStatusChange,
    RegisterResult, SignalService, SignalServiceServant, ViewerConnectionRequest,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::broadcast::Sender;
use webrtc::peer_connection::RTCSdpType;

/// SignalService state for IPC
/// Wrapped in Arc for shared ownership without cloning
pub struct IpcHostService {
    pub signal_url: String,
    pub reconnect_interval_secs: u64,
    pub username: String,
    pub password: String,
    pub signal_client: Arc<tokio::sync::RwLock<Option<SignalClient>>>,
    pub host_id: String,
    pub sessions: Arc<tokio::sync::Mutex<Vec<(String, Arc<RtcPeer>)>>>,
    pub capturer: Option<Arc<crate::capture::ScreenCapturer>>,
    pub stun: Vec<String>,
    pub fps: u32,
    /// Broadcast channel for peer status changes to all connected clients
    pub peer_status_broadcast:
        Arc<tokio::sync::broadcast::Sender<shared::ice0_service::PeerStatusChange>>,
    /// Broadcast channel for ICE candidates to all connected clients
    pub ice_candidate_broadcast:
        Arc<tokio::sync::broadcast::Sender<shared::ice0_service::IceCandidateInfo>>,
    /// Global ice0 Communicator shared between ipc_server and signal_client
    pub communicator: Arc<Communicator>,
}

/// Wrapper for IpcHostService to implement SignalService
/// This allows us to pass Arc<IpcHostService> to the servant
#[derive(Clone)]
pub struct IpcHostServiceWrapper(pub Arc<IpcHostService>);

/// ice0 SignalService implementation for the wrapper
#[async_trait::async_trait]
impl SignalService for IpcHostServiceWrapper {
    async fn user_login(&self, username: String, password: String, ctx: Current) -> LoginResult {
        self.0.user_login(username, password, ctx).await
    }

    async fn user_logout(&self, ctx: Current) {
        self.0.user_logout(ctx).await
    }

    async fn user_register(
        &self,
        username: String,
        password: String,
        email: String,
        ctx: Current,
    ) -> RegisterResult {
        self.0.user_register(username, password, email, ctx).await
    }

    async fn machine_list(&self, user_id: String, ctx: Current) -> Vec<MachineInfo> {
        self.0.machine_list(user_id, ctx).await
    }

    async fn machine_bind(&self, token: String, host_id: String, name: String, os: String, ctx: Current) {
        self.0.machine_bind(token, host_id, name, os, ctx, ).await
    }

    async fn machine_online(&self, host_id: String, name: String, os: String, ctx: Current) {
        self.0.machine_online(host_id, name, os, ctx, ).await
    }

    async fn machine_unbind(&self, token: String, host_id: String, ctx: Current) {
        self.0.machine_unbind(token, host_id, ctx, ).await
    }

    async fn viewer_connect_with_offer(
        &self,
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
        ctx: Current,
    ) -> Answer {
        self.0
            .viewer_connect_with_offer(viewer_id, host_id, password, offer_sdp, ctx, )
            .await
    }

    async fn ice_candidate(
        &self,
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
        ctx: Current,
    ) {
        self.0
            .ice_candidate(viewer_id, host_id, candidate, sdp_mid, sdp_mline_index, ctx)
            .await
    }

    async fn viewer_quality_update(
        &self,
        viewer_id: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32,
        ctx: Current,
    ) {
        self.0
            .viewer_quality_update(viewer_id, quality, max_dimension, fps, bitrate_kbps, ctx)
            .await
    }
}

/// ice0 SignalService implementation for IpcHostService
#[async_trait::async_trait]
impl SignalService for IpcHostService {
    async fn user_login(&self, username: String, password: String, _ctx: Current) -> LoginResult {
        info!("=== IPC SERVER: user_login called for user: {} ===", username);

        let client = match self.get_client().await {
            Some(c) => {
                info!("IPC SERVER: Got signal client, forwarding login");
                c
            },
            None => {
                error!("IPC SERVER: No signal client available!");
                return LoginResult {
                    success: false,
                    token: String::new(),
                    user_id: 0,
                    username: String::new(),
                    reason: "Not connected to signal server".to_string(),
                };
            }
        };

        info!("IPC SERVER: Calling client.user_login()...");
        match client.user_login(&username, &password).await {
            Ok(result) => {
                info!("IPC SERVER: Login succeeded: {}", result.success);
                result
            },
            Err(e) => {
                error!("IPC SERVER: Login failed: {}", e);
                LoginResult {
                    success: false,
                    token: String::new(),
                    user_id: 0,
                    username: String::new(),
                    reason: format!("Login failed: {}", e),
                }
            }
        }
    }

    async fn user_logout(&self, _ctx: Current) {
        info!("RPC: user_logout called");

        if let Some(client) = self.get_client().await {
            if let Err(e) = client.user_logout().await {
                error!("Logout failed: {}", e);
            }
        }
    }

    async fn user_register(
        &self,
        username: String,
        password: String,
        email: String,
        _ctx: Current,
    ) -> RegisterResult {
        info!("RPC: user_register called for user: {}", username);

        let client = match self.get_client().await {
            Some(c) => c,
            None => {
                return RegisterResult {
                    success: false,
                    user_id: 0,
                    message: "Not connected to signal server".to_string(),
                };
            }
        };

        match client.user_register(&username, &password, &email).await {
            Ok(result) => result,
            Err(e) => {
                error!("Registration failed: {}", e);
                RegisterResult {
                    success: false,
                    user_id: 0,
                    message: format!("Registration failed: {}", e),
                }
            }
        }
    }

    async fn machine_list(&self, token: String, _ctx: Current) -> Vec<MachineInfo> {
        info!("RPC: machine_list called with token");

        let client = match self.get_client().await {
            Some(c) => c,
            None => {
                warn!("No signal client available for machine_list");
                return vec![];
            }
        };

        match client.machine_list().await {
            Ok(list) => list,
            Err(e) => {
                error!("Failed to get machine list: {}", e);
                vec![]
            }
        }
    }

    async fn machine_bind(&self, token: String, host_id: String, name: String, os: String, _ctx: Current) {
        info!("RPC: machine_bind called for host: {} ({})", host_id, name);

        if let Some(client) = self.get_client().await {
            if let Err(e) = client.machine_bind(token, &host_id, &name, &os).await {
                error!("Failed to bind machine: {}", e);
            }
        }
    }

    async fn machine_online(&self, host_id: String, name: String, os: String, _ctx: Current) {
        info!(
            "RPC: machine_online called for host: {} ({})",
            host_id, name
        );

        if let Some(client) = self.get_client().await {
            if let Err(e) = client.machine_online(&host_id, &name, &os).await {
                error!("Failed to register machine: {}", e);
            }
        }
    }

    async fn machine_unbind(&self, token: String, machine_id: String, _ctx: Current) {
        info!("RPC: machine_unbind called for machine: {}", machine_id);

        if let Some(client) = self.get_client().await {
            if let Err(e) = client.machine_unbind(token, &machine_id).await {
                error!("Failed to unbind machine: {}", e);
            }
        }
    }

    async fn viewer_connect_with_offer(
        &self,
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
        _ctx: Current,
    ) -> Answer {
        info!(
            "RPC: viewer_connect_with_offer called from viewer {} to host {}",
            viewer_id, host_id
        );

        // Forward to Signal Server via signal_client
        match self.get_client().await {
            Some(client) => {
                info!(
                    "Forwarding viewer connection to Signal Server: {} -> {}",
                    viewer_id, host_id
                );
                match client
                    .viewer_connect_with_offer(&viewer_id, &host_id, &password, &offer_sdp)
                    .await
                {
                    Ok(answer) => {
                        info!(
                            "Received Answer from Signal Server for viewer {} (accepted: {})",
                            viewer_id, answer.accepted
                        );
                        answer
                    }
                    Err(e) => {
                        error!("Failed to forward to Signal Server: {}", e);
                        Answer {
                            accepted: false,
                            reason: format!("Failed to forward to Signal Server: {}", e),
                            answer: String::new(),
                        }
                    }
                }
            }
            None => {
                error!("Not connected to Signal Server");
                Answer {
                    accepted: false,
                    reason: "Not connected to Signal Server".to_string(),
                    answer: String::new(),
                }
            }
        }
    }

    async fn ice_candidate(
        &self,
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
        _ctx: Current,
    ) {
        info!(
            "RPC: ice_candidate called from {} to {}",
            viewer_id, host_id
        );

        if let Some(client) = self.get_client().await {
            if let Err(e) = client
                .ice_candidate(&viewer_id, &host_id, &candidate, sdp_mid, sdp_mline_index)
                .await
            {
                error!("Failed to forward ICE candidate: {}", e);
            }
        }
    }

    async fn viewer_quality_update(
        &self,
        viewer_id: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32,
        _ctx: Current,
    ) {
        info!(
            "RPC: viewer_quality_update called for viewer: {}",
            viewer_id
        );

        if let Some(client) = self.get_client().await {
            if let Err(e) = client
                .viewer_quality_update(&viewer_id, quality, max_dimension, fps, bitrate_kbps)
                .await
            {
                error!("Failed to update quality: {}", e);
            }
        }
    }
}

impl IpcHostService {
    pub async fn start_signal_client(self: &Arc<Self>) {
        let signal_url = self.signal_url.clone();
        let reconnect_interval = self.reconnect_interval_secs;
        let host_id = self.host_id.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let broadcast_tx = self.peer_status_broadcast.clone();
        let service = Arc::clone(self);

        info!("Starting signal client reconnection task");

        tokio::spawn(async move {
            loop {
                info!("Connecting to signal server: {}", signal_url);

                match SignalClient::connect(&signal_url, service.clone()).await {
                    Ok(client) => {
                        info!("Successfully connected to signal server");
                        service
                            .on_signal_connected(
                                client,
                                broadcast_tx.clone(),
                                &host_id,
                                &username,
                                &password,
                            )
                            .await;
                        // Connection established, break out of reconnection loop
                        // (reconnection will only happen if connection is lost)
                        break;
                    }
                    Err(e) => {
                        error!("Failed to connect: {}", e);
                        error!("Retrying in {} seconds...", reconnect_interval);
                        tokio::time::sleep(std::time::Duration::from_secs(reconnect_interval))
                            .await;
                    }
                }
            }
        });
    }

    async fn on_signal_connected(
        self: &Arc<Self>,
        client: SignalClient,
        broadcast_tx: Arc<Sender<PeerStatusChange>>,
        host_id: &String,
        username: &String,
        password: &String,
    ) {
        info!("Connected to signal server");

        // Store the signal client so it can be used by RPC handlers
        {
            let mut guard = self.signal_client.write().await;
            *guard = Some(client);
            info!("Signal client stored and available for RPC forwarding");
        }

        // Get a reference to the stored client for setting up callbacks
        let client = match self.get_client().await {
            Some(c) => c,
            None => {
                error!("Failed to retrieve stored signal client");
                return;
            }
        };

        // Set up peer status callback
        let client_clone = client.clone();
        client_clone.set_peer_status_callback(move |change| {
            info!(
                "Peer status change: {} is now {}",
                change.node_id,
                if change.online { "online" } else { "offline" }
            );
            let _ = broadcast_tx.send(change);
        });

        // Set up ICE candidate callback - forward to clients
        let ice_broadcast = self.ice_candidate_broadcast.clone();
        let client_clone = client.clone();
        client_clone.set_ice_candidate_callback(move |ice_info| {
            info!("Forwarding ICE candidate from signal server to clients");
            let _ = ice_broadcast.send(ice_info);
        });

        let service = Arc::clone(self);
        // Set up viewer connection request callback
        let client_clone = client.clone();
        client_clone.set_viewer_request_callback(move |request| {
            let request_clone = request.clone();
            let service = Arc::clone(&service);
            // Block on the async operation to return Answer synchronously
            match tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    service
                        .handle_viewer_connection_request(request_clone)
                        .await
                })
            }) {
                Ok(answer) => answer,
                Err(e) => {
                    error!("Failed to handle viewer connection request: {}", e);
                    Answer {
                        accepted: false,
                        reason: e.to_string(),
                        answer: String::new(),
                    }
                }
            }
        });

        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "Unknown".to_string());

        let os_info = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);

        info!("Registering machine as online: {} ({})", hostname, host_id);
        if let Err(e) = client.machine_online(&host_id, &hostname, &os_info).await {
            warn!("Failed to register machine: {}", e);
            return;
        } else {
            info!("Machine registered as online successfully");
        }

        // Note: No subscription needed! ice0's bidirectional RPC means the signal server
        // can directly call HostCallback.request_control_from_viewer() when viewers connect.
        info!("Host is ready to receive viewer requests via bidirectional RPC callbacks");
    }

    pub async fn get_client(&self) -> Option<Arc<SignalClient>> {
        let guard = self.signal_client.read().await;
        // Note: SignalClient doesn't need to be cloned here
        // We'll use Arc<SignalClient> directly in the future
        guard.as_ref().map(|c| Arc::new(c.clone()))
    }

    /// Start the IPC server using ice0 over TCP localhost
    pub async fn start_ipc_server(self: &Arc<Self>, socket_path: &str) -> anyhow::Result<()> {
        info!("IPC server (ice0) starting on: {}", socket_path);

        // Use the shared communicator from IpcHostService
        let communicator = self.communicator.clone();

        // Parse socket_path to extract host and port
        // Expected format: "tcp://127.0.0.1:9010" or "127.0.0.1:9010"
        let endpoint_str = socket_path
            .trim_start_matches("tcp://")
            .trim_start_matches("ws://");
        
        // Split into host and port
        let parts: Vec<&str> = endpoint_str.split(':').collect();
        let (host, port) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("127.0.0.1", "9010")  // Default fallback
        };

        // Create ObjectAdapter with TCP endpoint
        let adapter = communicator
            .create_object_adapter(
                "HostService",
                Some(format!("tcp -h {} -p {}", host, port)),
                None,
                None,
            )
            .await?;

        // Create servant for this service using the wrapper
        // Note: The identity must match what the client uses in its proxy string
        // Client uses "SignalService" because this implements the SignalService trait
        let service_identity = Identity::new("SignalService");
        let service_servant = Arc::new(SignalServiceServant::new(IpcHostServiceWrapper(
            Arc::clone(self),
        )));
        adapter
            .add_servant(service_identity.clone(), service_servant)
            .await;
        adapter.activate().await?;

        info!(
            "ice0 ObjectAdapter activated for SignalService on: {}",
            socket_path
        );
        info!("IPC server (ice0) ready to accept connections");

        Ok(())
    }

    // ===== Handler implementations =====
    async fn handle_viewer_connect_with_offer(
        self: &Arc<Self>,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let (viewer_id, host_id, password, offer_sdp): (String, String, String, String) =
            match params {
                Some(p) => serde_json::from_value(p)?,
                None => anyhow::bail!("Missing parameters"),
            };

        info!(
            "IPC viewer_connect_with_offer: {} -> {} (SDP length: {})",
            viewer_id,
            host_id,
            offer_sdp.len()
        );

        match self.get_client().await {
            Some(client) => match client
                .viewer_connect_with_offer(&viewer_id, &host_id, &password, &offer_sdp)
                .await
            {
                Ok(result) => Ok(serde_json::to_value(result)?),
                Err(e) => Ok(serde_json::to_value(Answer {
                    accepted: false,
                    reason: format!("Forwarding failed: {}", e),
                    answer: String::new(),
                })?),
            },
            None => Ok(serde_json::to_value(Answer {
                accepted: false,
                reason: "Not connected to signal server".to_string(),
                answer: String::new(),
            })?),
        }
    }

    async fn handle_ice_candidate(
        self: &Arc<Self>,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let raw: serde_json::Value = match params {
            Some(p) => p,
            None => anyhow::bail!("Missing parameters"),
        };

        let viewer_id = raw[0].as_str().unwrap_or("").to_string();
        let host_id = raw[1].as_str().unwrap_or("").to_string();
        let candidate = raw[2].as_str().unwrap_or("").to_string();
        let sdp_mid = raw[3].as_str().map(|s| s.to_string());
        let sdp_mline_index = raw[4].as_u64().map(|v| v as u16);

        info!("IPC ice_candidate from: {} to: {}", viewer_id, host_id);

        match self.get_client().await {
            Some(client) => {
                client
                    .ice_candidate(
                        &viewer_id,
                        &host_id,
                        &candidate,
                        sdp_mid.clone(),
                        sdp_mline_index,
                    )
                    .await?;
                Ok(serde_json::Value::Null)
            }
            None => anyhow::bail!("Not connected"),
        }
    }

    async fn handle_viewer_quality_update(
        self: &Arc<Self>,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let raw: serde_json::Value = match params {
            Some(p) => p,
            None => anyhow::bail!("Missing parameters"),
        };

        let viewer_id = raw[0].as_str().unwrap_or("").to_string();
        let quality = raw[1].as_u64().unwrap_or(0) as u32;
        let max_dimension = raw[2].as_u64().unwrap_or(0) as u32;
        let fps = raw[3].as_u64().unwrap_or(0) as u32;
        let bitrate_kbps = raw[4].as_u64().unwrap_or(0) as u32;

        info!("IPC viewer_quality_update: {}", viewer_id);

        match self.get_client().await {
            Some(client) => {
                client
                    .viewer_quality_update(&viewer_id, quality, max_dimension, fps, bitrate_kbps)
                    .await?;
                Ok(serde_json::Value::Null)
            }
            None => anyhow::bail!("Not connected"),
        }
    }

    /// Handle incoming viewer connection request from signal server
    /// Returns Answer to be sent back to viewer via Signal Server callback
    pub async fn handle_viewer_connection_request(
        self: &Arc<Self>,
        request: ViewerConnectionRequest,
    ) -> anyhow::Result<Answer> {
        info!(
            "Handling viewer connection request: {} -> {}",
            request.viewer_id, request.host_id
        );

        // Verify password against configured host password
        // The password is stored in service.password (from config file)
        // If password is not configured (empty), accept all connections
        let host_password = &self.password;
        if !host_password.is_empty() && request.password != *host_password {
            warn!(
                "Viewer {} failed password authentication for host {}",
                request.viewer_id, request.host_id
            );
            // Send rejection - but we don't have a direct way to notify viewer
            // The viewer will timeout waiting for answer
            anyhow::bail!("Password authentication failed");
        }

        info!(
            "Viewer {} password verified successfully",
            request.viewer_id
        );

        let peer_id = request.viewer_id.clone();
        let stun_servers = self.stun.clone();

        // Check if viewer sent an offer (viewer is offerer)
        if let Some(offer_sdp) = request.offer_sdp {
            info!(
                "Viewer {} sent Offer (SDP length: {}), creating Answer",
                request.viewer_id,
                offer_sdp.len()
            );

            // Create RtcPeer for this viewer
            let mut peer = RtcPeer::new_host(&peer_id, &stun_servers).await?;

            // Attach capturer if available
            if let Some(capturer) = &self.capturer {
                peer.attach_capturer(capturer.clone())?;
                info!("Attached capturer to peer: {}", peer_id);
            } else {
                warn!("No capturer available for peer: {}", peer_id);
            }

            // Set remote description (viewer's offer)
            peer.set_remote_description(&offer_sdp, RTCSdpType::Offer)
                .await?;
            info!("Set remote description for viewer: {}", peer_id);

            // Create answer
            let answer = peer.create_answer().await?;
            info!("Created Answer for viewer: {}", peer_id);

            // Store peer in sessions
            {
                let mut sessions = self.sessions.lock().await;
                sessions.push((peer_id.clone(), Arc::new(peer)));
                info!(
                    "Stored peer session: {} (total: {})",
                    peer_id,
                    sessions.len()
                );
            }

            // Return Answer to be sent back via callback
            return Ok(Answer {
                accepted: true,
                reason: String::new(),
                answer: answer.sdp,
            });
        } else {
            // Old flow: viewer expects host to create offer
            // This is deprecated but kept for backward compatibility
            info!(
                "Viewer {} expects Host to create Offer (deprecated flow)",
                request.viewer_id
            );

            // Create RtcPeer for this viewer
            let mut peer = RtcPeer::new_host(&peer_id, &stun_servers).await?;

            // Attach capturer if available
            if let Some(capturer) = &self.capturer {
                peer.attach_capturer(capturer.clone())?;
                info!("Attached capturer to peer: {}", peer_id);
            } else {
                warn!("No capturer available for peer: {}", peer_id);
            }

            // Create offer
            let offer = peer.create_offer().await?;
            info!("Created Offer for viewer: {}", peer_id);

            // Store peer in sessions
            {
                let mut sessions = self.sessions.lock().await;
                sessions.push((peer_id.clone(), Arc::new(peer)));
                info!(
                    "Stored peer session: {} (total: {})",
                    peer_id,
                    sessions.len()
                );
            }

            // Return Offer (deprecated flow)
            return Ok(Answer {
                accepted: true,
                reason: String::new(),
                answer: offer.sdp,
            });
        }

        unreachable!("All branches should return")
    }
}
