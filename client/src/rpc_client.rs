//! ice0 RPC client for communicating with host-service.
//!
//! This client uses ice0 RPC framework over interprocess sockets (Unix) or named pipes (Windows)
//! for bidirectional communication between the client GUI and host-service.
//!
//! Features:
//! - Type-safe RPC calls via SignalServicePrx
//! - Bidirectional callbacks via ClientCallback
//! - Automatic reconnection support
//! - Concurrent request handling

use ice0::prelude::*;
use log::{error, info, warn};
use shared::ice0_service::{
    Answer, ClientCallback, ClientCallbackServant, LoginResult, MachineInfo, RegisterResult,
    SignalServicePrx,
};
use std::sync::Arc;

use crate::client_callback::ClientCallbackImpl;

/// ice0 RPC client that communicates with host-service
#[derive(Clone)]
pub struct RpcClient {
    /// Signal service proxy for making RPC calls
    signal_proxy: Option<SignalServicePrx>,
    /// Client callback implementation for receiving notifications
    client_callback: Arc<ClientCallbackImpl>,
    /// ice0 communicator
    communicator: Option<Arc<Communicator>>,
    /// Socket path
    socket_path: String,
    /// Authentication token (set after login)
    token: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl RpcClient {
    /// Create a new RPC client
    pub fn new() -> Self {
        Self {
            signal_proxy: None,
            client_callback: Arc::new(ClientCallbackImpl::new()),
            communicator: None,
            socket_path: "tcp://127.0.0.1:9010".to_string(),  // Use TCP localhost for IPC (same as host-service)
            token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Get the client callback for registering handlers
    pub fn client_callback(&self) -> Arc<ClientCallbackImpl> {
        self.client_callback.clone()
    }

    /// Connect to host-service via ice0
    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Connecting to host-service ice0 socket: {}", self.socket_path);

        // Create ice0 Communicator
        let communicator = Communicator::default()
            .await
            .map_err(|e| format!("Failed to create communicator: {}", e))?;

        info!("✓ ice0 Communicator created");

        // Create ObjectAdapter for callbacks (no endpoints - callback-only)
        let callback_adapter = communicator
            .create_object_adapter("ClientCallbackAdapter", None, None, None)
            .await
            .map_err(|e| format!("Failed to create callback adapter: {}", e))?;

        info!("✓ Callback adapter created");

        // Register ClientCallback servant
        let callback_identity = Identity::new("ClientCallback");
        let callback_servant = Arc::new(ClientCallbackServant::new((*self.client_callback).clone()));
        callback_adapter
            .add_servant(callback_identity.clone(), callback_servant.clone())
            .await;

        info!("✓ Callback servant registered");

        // Activate adapter
        callback_adapter.activate().await;
        info!("✓ Callback adapter activated");

        // Create proxy to HostService via TCP localhost
        // Parse socket_path: "tcp://127.0.0.1:9010" → "tcp -h 127.0.0.1 -p 9010"
        let endpoint_str = self.socket_path
            .trim_start_matches("tcp://")
            .trim_start_matches("ws://");
        
        let parts: Vec<&str> = endpoint_str.split(':').collect();
        let (host, port) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("127.0.0.1", "9010")  // Default fallback
        };
        
        // Note: The IPC server on the host-service registers as "SignalService" (not "HostService")
        // because it implements the SignalService trait via IpcHostServiceWrapper
        let proxy_string = format!("SignalService:tcp -h {} -p {}", host, port);
        let proxy = communicator
            .string_to_proxy(&proxy_string)
            .await
            .map_err(|e| format!("Failed to create proxy: {}", e))?;

        info!("✓ Proxy created: {}", proxy_string);

        // Get connection from proxy
        let connection = match proxy.ice_getConnection().await {
            Ok(connection) => connection,
            Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
        };

        // Set callback adapter to connection
        connection.set_adapter(Some(callback_adapter.clone())).await;
        info!("✓ Callback adapter set to connection");

        // Register servant to adapter via multiplexer
        connection
            .add_servant(callback_identity.clone(), callback_servant)
            .await;
        info!("✓ Callback servant registered to connection");

        // Create SignalService proxy
        let signal_proxy = SignalServicePrx::checked_cast(proxy)
            .map_err(|e| format!("Failed to cast proxy: {}", e))?;

        self.signal_proxy = Some(signal_proxy);
        self.communicator = Some(communicator);

        info!("Successfully connected to host-service via ice0");
        Ok(())
    }

    /// Disconnect from host-service
    pub async fn disconnect(&mut self) {
        info!("Disconnecting from host-service");
        self.signal_proxy = None;
        self.communicator = None;
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.signal_proxy.is_some()
    }

    // ─── SignalService RPC Methods ─────────────────────────────────────────

    /// Authenticate user with signal server
    pub async fn user_login(
        &self,
        username: String,
        password: String,
    ) -> Result<LoginResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: user_login({})", username);
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        let result = proxy
            .user_login(username, password)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        
        // Store token after successful login
        if result.success {
            *self.token.lock().await = Some(result.token.clone());
            info!("Login successful, token stored for user {}", result.user_id);
        }
        
        Ok(result)
    }

    /// Logout current user
    pub async fn user_logout(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: user_logout");
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .user_logout()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Register a new user
    pub async fn user_register(
        &self,
        username: String,
        password: String,
        email: String,
    ) -> Result<RegisterResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: user_register({})", username);
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .user_register(username, password, email)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Request the list of online machines for the authenticated user
    pub async fn machine_list(&self) -> Result<Vec<MachineInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.token.lock().await;
        let token_str = token.as_ref().ok_or("No authentication token available")?;
        info!("RPC: machine_list with token");
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .machine_list(token_str.to_string())
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Bind a machine to current user
    pub async fn machine_bind(
        &self,
        host_id: String,
        name: String,
        os: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let token = self.token.lock().await;
        let token_str = token.as_ref().ok_or("No authentication token available")?;
        info!("RPC: machine_bind with token");
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .machine_bind(token_str.to_string(), host_id, name, os)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Register a machine (online notification)
    pub async fn machine_online(
        &self,
        host_id: String,
        name: String,
        os: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: machine_online({}, {}, {})", host_id, name, os);
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .machine_online(host_id, name, os)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Unbind a machine from current user
    pub async fn machine_unbind(&self, host_id: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let token = self.token.lock().await;
        let token_str = token.as_ref().ok_or("No authentication token available")?;
        info!("RPC: machine_unbind with token");
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .machine_unbind(token_str.to_string(), host_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Handle viewer connection request with Offer SDP
    pub async fn viewer_connect_with_offer(
        &self,
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
    ) -> Result<Answer, Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: viewer_connect_with_offer({}, {})", viewer_id, host_id);
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .viewer_connect_with_offer(viewer_id, host_id, password, offer_sdp)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Send viewer's ICE candidate to host
    pub async fn ice_candidate(
        &self,
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: ice_candidate({} -> {}, len={})", viewer_id, host_id, candidate.len());
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .ice_candidate(viewer_id, host_id, candidate, sdp_mid, sdp_mline_index)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Update streaming quality for a viewer
    pub async fn viewer_quality_update(
        &self,
        viewer_id: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("RPC: viewer_quality_update({}, q={}, dim={}, fps={}, bitrate={})", 
              viewer_id, quality, max_dimension, fps, bitrate_kbps);
        let proxy = self
            .signal_proxy
            .as_ref()
            .ok_or("Not connected to host-service")?;
        proxy
            .viewer_quality_update(viewer_id, quality, max_dimension, fps, bitrate_kbps)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}
