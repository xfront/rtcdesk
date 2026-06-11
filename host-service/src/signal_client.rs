//! ice0-based signal client for host-service

use ice0::prelude::*;
use shared::ice0_service::{SignalServicePrx, HostCallback, HostCallbackServant, HostCallbackPrx, SignalService, LoginResult, RegisterResult, MachineInfo, Answer, PeerStatusChange, ViewerConnectionRequest, IceCandidateInfo};
use log::{info, warn, error};
use std::sync::Arc;
use std::sync::Mutex;

/// HostCallback implementation for receiving callbacks from Signal Server
#[derive(Clone)]
struct HostCallbackImpl {
    peer_status_callback: Arc<Mutex<Option<Box<dyn Fn(PeerStatusChange) + Send + Sync>>>>,
    viewer_request_callback: Arc<Mutex<Option<Box<dyn Fn(ViewerConnectionRequest) -> Answer + Send + Sync>>>>,
    ice_candidate_callback: Arc<Mutex<Option<Box<dyn Fn(IceCandidateInfo) + Send + Sync>>>>,
    // Reference to IpcHostService for creating WebRTC Answer
    ipc_server: Arc<crate::ipc_server::IpcHostService>,
}

#[async_trait::async_trait]
impl HostCallback for HostCallbackImpl {
    async fn on_peer_status_change(
        &self,
        change: PeerStatusChange,
        _ctx: Current,
    ) {
        info!("Received peer status change callback: {} is now {}", 
              change.node_id, 
              if change.online { "online" } else { "offline" });
        
        if let Some(callback) = self.peer_status_callback.lock().unwrap().as_ref() {
            callback(change);
        }
    }

    async fn request_control_from_viewer(
        &self,
        request: ViewerConnectionRequest,
        _ctx: Current,
    ) -> Answer {
        info!("Received viewer connection request callback: {} -> {}",
              request.viewer_id, request.host_id);
        
        // Use ipc_server to create Answer
        match self.ipc_server.handle_viewer_connection_request(request.clone()).await {
            Ok(answer) => {
                info!("Successfully created Answer for viewer {}", request.viewer_id);
                answer
            }
            Err(e) => {
                error!("Failed to handle viewer connection request: {}", e);
                Answer {
                    accepted: false,
                    reason: e.to_string(),
                    answer: String::new(),
                }
            }
        }
    }

    async fn on_ice_candidate(
        &self,
        info: IceCandidateInfo,
        _ctx: Current,
    ) {
        info!("Received ICE candidate callback from viewer {} to host {}",
              info.from, info.to);
        
        if let Some(callback) = self.ice_candidate_callback.lock().unwrap().as_ref() {
            callback(info);
        } else {
            warn!("No ICE candidate callback registered");
        }
    }
}

/// Signal client using ice0 TCP with bidirectional support
#[derive(Clone)]
pub struct SignalClient {
    rpc_client: SignalServicePrx,
    token: Arc<Mutex<Option<String>>>,
    /// Callback adapter for receiving callbacks from server
    callback_adapter: Arc<ObjectAdapter>,
    /// Callback for peer status changes
    peer_status_callback: Arc<Mutex<Option<Box<dyn Fn(PeerStatusChange) + Send + Sync>>>>,
    /// Callback for viewer connection requests
    viewer_request_callback: Arc<Mutex<Option<Box<dyn Fn(ViewerConnectionRequest) -> Answer + Send + Sync>>>>,
    /// Callback for ICE candidates from viewers
    ice_candidate_callback: Arc<Mutex<Option<Box<dyn Fn(IceCandidateInfo) + Send + Sync>>>>,
}

impl SignalClient {
    /// Connect to signal server via ice0 TCP with bidirectional support
    pub async fn connect(
        url: &str,
        ipc_server: Arc<crate::ipc_server::IpcHostService>
    ) -> anyhow::Result<Self> {
        info!("Connecting to signal server: {}", url);
        
        // Use the shared communicator from IpcHostService
        let communicator = ipc_server.communicator.clone();
        
        // Extract hostname from URL (remove protocol prefix if present)
        let hostname = url
            .trim_start_matches("ws://")
            .trim_start_matches("wss://")
            .trim_start_matches("tcp://")
            .split(':')
            .next()
            .unwrap_or(url);
        
        // Convert "localhost" to "127.0.0.1" for ice0 TCP endpoint
        // ice0 doesn't resolve hostnames, it needs IP addresses
        let ip_addr = if hostname == "localhost" {
            "127.0.0.1"
        } else {
            hostname
        };
        
        let proxy_string = format!("SignalService:tcp -h {} -p 9009", ip_addr);
        info!("Creating proxy with string: {}", proxy_string);
        
        // Create proxy to remote SignalService
        let proxy = communicator.string_to_proxy(&proxy_string).await?;
        let rpc_client = SignalServicePrx::checked_cast(proxy)?;
        
        info!("Connected to signal server");
        
        // Create callback adapter for receiving callbacks from server
        let callback_adapter = communicator.create_object_adapter("HostCallbackAdapter", None, None, None).await?;
        
        // Create callback implementation
        let peer_callback_clone = Arc::new(Mutex::new(None));
        let viewer_callback_clone = Arc::new(Mutex::new(None));
        let ice_callback_clone = Arc::new(Mutex::new(None));
        
        let callback_impl = HostCallbackImpl {
            peer_status_callback: peer_callback_clone.clone(),
            viewer_request_callback: viewer_callback_clone.clone(),
            ice_candidate_callback: ice_callback_clone.clone(),
            ipc_server: ipc_server.clone(),
        };
        
        // Register callback servant
        let callback_identity = Identity::new("HostCallback");
        let servant = Arc::new(HostCallbackServant::new(callback_impl));
        callback_adapter.add_servant(callback_identity.clone(), servant.clone()).await;
        callback_adapter.activate().await;
        
        // Get connection and set callback adapter
        let connection = rpc_client.to_proxy().ice_getConnection().await?;
        connection.set_adapter(Some(callback_adapter.clone())).await;
        connection.add_servant(callback_identity, servant).await;
        
        let client = Self {
            rpc_client,
            token: Arc::new(Mutex::new(None)),
            callback_adapter,
            peer_status_callback: peer_callback_clone,
            viewer_request_callback: viewer_callback_clone,
            ice_candidate_callback: ice_callback_clone,
        };
        
        info!("Bidirectional signal client initialized");
        Ok(client)
    }

    /// Login
    pub async fn user_login(&self, username: &str, password: &str) -> anyhow::Result<LoginResult> {
        info!("Signal client: forwarding user_login for '{}' to signal server", username);
        
        let result = self.rpc_client.user_login(
            username.to_string(),
            password.to_string()
        ).await?;
        
        info!("Signal client: received login response from signal server: success={}", result.success);
        
        if result.success {
            if let Ok(mut token) = self.token.lock() {
                *token = Some(result.token.clone());
            }
        }
        
        Ok(result)
    }
    
    /// Logout
    pub async fn user_logout(&self) -> anyhow::Result<()> {
        self.rpc_client.user_logout().await?;
        if let Ok(mut token) = self.token.lock() {
            *token = None;
        }
        Ok(())
    }

    /// Register
    pub async fn user_register(&self, username: &str, password: &str, email: &str) -> anyhow::Result<RegisterResult> {
        let result = self.rpc_client.user_register(
            username.to_string(),
            password.to_string(),
            email.to_string()
        ).await?;
        Ok(result)
    }

    /// Register machine with signal server
    pub async fn machine_online(&self, host_id: &str, name: &str, os: &str) -> anyhow::Result<()> {
        info!("Registering machine: {} ({})", name, host_id);
        self.rpc_client.machine_online(
            host_id.to_string(),
            name.to_string(),
            os.to_string()
        ).await?;
        info!("Machine registered successfully");
        
        Ok(())
    }

    /// Bind machine
    pub async fn machine_bind(&self, token: String, host_id: &str, name: &str, os: &str) -> anyhow::Result<()> {
        self.rpc_client.machine_bind(
            token,
            host_id.to_string(),
            name.to_string(),
            os.to_string()
        ).await?;
        Ok(())
    }
    
    /// Unbind machine
    pub async fn machine_unbind(&self, token: String, host_id: &str) -> anyhow::Result<()> {
        self.rpc_client.machine_unbind(
            token,
            host_id.to_string()
        ).await?;
        Ok(())
    }

    /// Get machine list for the authenticated user
    pub async fn machine_list(&self) -> anyhow::Result<Vec<MachineInfo>> {
        let token = self.token.lock()
            .ok()
            .and_then(|t| t.clone())
            .ok_or_else(|| anyhow::anyhow!("No authentication token available"))?;
        let machines = self.rpc_client.machine_list(token).await?;
        Ok(machines)
    }

    /// Viewer connect with offer
    /// 
    /// This is the ONLY method needed for WebRTC connection setup.
    /// The viewer creates an Offer SDP and sends it to the host.
    /// The host validates the password, creates an Answer SDP, and returns it.
    pub async fn viewer_connect_with_offer(
        &self,
        viewer_id: &str,
        host_id: &str,
        password: &str,
        offer_sdp: &str
    ) -> anyhow::Result<Answer> {
        let result = self.rpc_client.viewer_connect_with_offer(
            viewer_id.to_string(),
            host_id.to_string(),
            password.to_string(),
            offer_sdp.to_string()
        ).await?;
        Ok(result)
    }

    /// Send viewer offer (backward compatibility - deprecated)
    /// This is used by handle_viewer_connection_request to send Answer back
    pub async fn viewer_offer(&self, viewer_id: &str, host_id: &str, sdp: &str) -> anyhow::Result<()> {
        // TODO: This should use a callback mechanism to return Answer to waiting viewer
        // For now, just log it - the actual answer delivery needs bidirectional callback
        warn!("viewer_offer called (deprecated): {} -> {} (SDP length: {})", 
              viewer_id, host_id, sdp.len());
        Ok(())
    }

    /// Send ICE candidate
    pub async fn ice_candidate(
        &self,
        viewer_id: &str,
        host_id: &str,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>
    ) -> anyhow::Result<()> {
        self.rpc_client.ice_candidate(
            viewer_id.to_string(),
            host_id.to_string(),
            candidate.to_string(),
            sdp_mid,
            sdp_mline_index,
        ).await?;
        Ok(())
    }

    /// Update viewer quality
    pub async fn viewer_quality_update(
        &self,
        viewer_id: &str,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32
    ) -> anyhow::Result<()> {
        self.rpc_client.viewer_quality_update(
            viewer_id.to_string(),
            quality,
            max_dimension,
            fps,
            bitrate_kbps
        ).await?;
        Ok(())
    }

    /// Set peer status callback
    pub fn set_peer_status_callback<F>(&self, callback: F)
    where
        F: Fn(PeerStatusChange) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.peer_status_callback.lock() {
            *guard = Some(Box::new(callback));
        }
    }

    /// Set viewer request callback
    pub fn set_viewer_request_callback<F>(&self, callback: F)
    where
        F: Fn(ViewerConnectionRequest) -> Answer + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.viewer_request_callback.lock() {
            *guard = Some(Box::new(callback));
        }
    }

    /// Set ICE candidate callback
    pub fn set_ice_candidate_callback<F>(&self, callback: F)
    where
        F: Fn(IceCandidateInfo) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.ice_candidate_callback.lock() {
            *guard = Some(Box::new(callback));
        }
    }

    /// Start peer status subscription (spawns background task)
    pub fn start_peer_status_subscription(&self, _user_id: i64) {
        // Note: ice0 doesn't support native subscriptions
        // This would need a separate subscription connection or polling mechanism
        // For now, this is a placeholder
        warn!("Peer status subscription not yet implemented for ice0");
    }

    /// Start viewer request subscription (spawns background task)
    pub fn start_viewer_request_subscription(&self, _host_id: String) {
        // Note: ice0 doesn't support native subscriptions
        // This would need a separate subscription connection or polling mechanism
        // For now, this is a placeholder
        warn!("Viewer request subscription not yet implemented for ice0");
    }
}
