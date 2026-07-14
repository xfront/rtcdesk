//! RPC implementation for Signal Server using ice0.
//!
//! Implements the SignalService trait with ice0,
//! handling authentication, machine management, and WebRTC signaling.

use std::sync::Arc;
use std::cell::RefCell;
use ice0::prelude::*;
use ice0::current::Current;
use log::{error, info, warn};

use shared::ice0_service::{SignalService, Answer, LoginResult, MachineInfo, RegisterResult, HostCallbackPrx, ViewerConnectionRequest, IceCandidateInfo};

use crate::auth::AuthService;
use crate::machine::PeerRegistry;
use crate::connection::ConnectionManager;
use crate::subscription::SubscriptionManager;
use crate::cluster::ClusterManager;

// Thread-local storage for RequestContext (used for bidirectional callbacks)
thread_local! {
    static CURRENT_REQUEST_CONTEXT: RefCell<Option<ice0::adapter::RequestContext>> = RefCell::new(None);
}

/// Set the current request context (called by ice0 dispatch)
pub fn set_current_request_context(ctx: Option<ice0::adapter::RequestContext>) {
    CURRENT_REQUEST_CONTEXT.with(|c| {
        *c.borrow_mut() = ctx;
    });
}

/// Get the current request context
pub fn get_current_request_context() -> Option<ice0::adapter::RequestContext> {
    CURRENT_REQUEST_CONTEXT.with(|c| c.borrow().clone())
}

/// Signal Server RPC implementation.
#[derive(Clone)]
pub struct SignalServiceImpl {
    pub auth: Arc<AuthService>,
    pub machines: Arc<PeerRegistry>,
    pub connections: Arc<ConnectionManager>,
    pub subscriptions: Arc<SubscriptionManager>,
    /// Optional cluster manager for multi-node deployment
    pub cluster: Option<Arc<ClusterManager>>,
}

impl SignalServiceImpl {
    /// Validate token and extract user_id
    /// Returns user_id if token is valid, None otherwise
    fn validate_token(&self, token: &str) -> Option<i64> {
        match self.auth.verify_token(token) {
            Ok(claims) => {
                // Parse user_id from claims.sub
                claims.sub.parse::<i64>().ok()
            }
            Err(e) => {
                warn!("Token validation failed: {}", e);
                None
            }
        }
    }

    /// Handle a viewer connection request for a locally-connected host
    async fn handle_local_viewer_connection(
        &self,
        viewer_id: &str,
        host_id: &str,
        password: &str,
        offer_sdp: &str,
    ) -> Answer {
        // Get machine info from database
        match self.machines.db.get_machine_by_node_id(host_id).await {
            Ok(Some(machine)) => {
                if !machine.online {
                    return Answer {
                        accepted: false,
                        reason: "Machine is offline".to_string(),
                        answer: String::new(),
                    };
                }
                
                info!("Forwarding viewer connection to host {} with offer", host_id);
                
                // Create connection request with offer SDP
                let request = ViewerConnectionRequest {
                    viewer_id: viewer_id.to_string(),
                    host_id: host_id.to_string(),
                    password: password.to_string(),
                    offer_sdp: Some(offer_sdp.to_string()),
                };
                
                // Get stored HostCallbackPrx and invoke request_control_from_viewer
                if let Some(host_info) = self.connections.get_host_info(host_id).await {
                    info!("Invoking request_control_from_viewer on host {} via HostCallbackPrx", host_id);
                    match host_info.host_prx.request_control_from_viewer(request).await {
                        Ok(answer) => {
                            info!("Received Answer from host {}", host_id);
                            answer
                        }
                        Err(e) => {
                            error!("Failed to call host {} callback: {}", host_id, e);
                            Answer {
                                accepted: false,
                                reason: format!("Host callback failed: {}", e),
                                answer: String::new(),
                            }
                        }
                    }
                } else {
                    warn!("Host {} not found in connections", host_id);
                    Answer {
                        accepted: false,
                        reason: "Host not registered".to_string(),
                        answer: String::new(),
                    }
                }
            }
            Ok(None) => {
                Answer {
                    accepted: false,
                    reason: "Machine not found".to_string(),
                    answer: String::new(),
                }
            }
            Err(e) => {
                error!("Failed to query machine: {}", e);
                Answer {
                    accepted: false,
                    reason: "Internal server error".to_string(),
                    answer: String::new(),
                }
            }
        }
    }

    /// Handle an incoming cross-node viewer connection request (called by cluster handler)
    pub async fn handle_incoming_viewer_request(
        &self,
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
    ) -> Answer {
        info!("Handling incoming cross-node viewer request: {} -> {}", viewer_id, host_id);
        self.handle_local_viewer_connection(&viewer_id, &host_id, &password, &offer_sdp).await
    }

    /// Handle an incoming cross-node ICE candidate (called by cluster handler)
    pub async fn handle_incoming_ice_candidate(
        &self,
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) {
        info!("Handling incoming cross-node ICE candidate: {} -> {} (len={})", viewer_id, host_id, candidate.len());
        if let Some(host_info) = self.connections.get_host_info(&host_id).await {
            let info = IceCandidateInfo {
                from: viewer_id,
                to: host_id.clone(),
                candidate,
                sdp_mid,
                sdp_mline_index,
            };
            if let Err(e) = host_info.host_prx.on_ice_candidate(info).await {
                error!("Failed to forward ICE candidate to host {}: {}", host_id, e);
            }
        } else {
            warn!("Host {} not found locally for ICE candidate delivery", host_id);
        }
    }

    /// Handle a peer status broadcast from another node (called by cluster handler)
    pub async fn handle_remote_peer_status_change(&self, change: shared::ice0_service::PeerStatusChange) {
        info!("Received remote peer status broadcast: {} is {}", change.node_id, if change.online { "online" } else { "offline" });
        // Look up the machine in our local DB to find the user_id for local subscription delivery
        if let Ok(Some(machine)) = self.machines.db.get_machine_by_node_id(&change.node_id).await {
            if let Some(user_id) = machine.user_id {
                self.subscriptions.broadcast_peer_status(user_id, change).await;
            }
        }
    }
}

#[async_trait::async_trait]
impl SignalService for SignalServiceImpl {
    async fn user_login(&self, username: String, password: String, _current: Current) -> LoginResult {
        info!("=== SIGNAL SERVER: RPC login request for user: {} ===", username);
        
        match self.auth.login(&username, &password).await {
            Ok((user_id, token)) => {
                info!("User '{}' authenticated successfully (id={})", username, user_id);
                LoginResult {
                    success: true,
                    user_id,
                    token,
                    username: username.clone(),
                    reason: String::new(),
                }
            }
            Err(e) => {
                warn!("Login failed for '{}': {}", username, e);
                LoginResult {
                    success: false,
                    user_id: 0,
                    token: String::new(),
                    username: String::new(),
                    reason: e.to_string(),
                }
            }
        }
    }

    async fn user_logout(&self, _current: Current) {
        info!("RPC user_logout request");
        // Logout is handled by closing the connection
    }

    async fn user_register(&self, username: String, password: String, _email: String, _current: Current) -> RegisterResult {
        info!("RPC register request for user: {}", username);
        
        match self.auth.register(&username, &password).await {
            Ok((_uid, _token)) => {
                info!("User '{}' registered successfully", username);
                RegisterResult {
                    success: true,
                    user_id: _uid,
                    message: format!("User '{}' registered successfully", username),
                }
            }
            Err(e) => {
                warn!("Registration failed for '{}': {}", username, e);
                RegisterResult {
                    success: false,
                    user_id: 0,
                    message: e.to_string(),
                }
            }
        }
    }

    async fn machine_list(&self, token: String, _current: Current) -> Vec<MachineInfo> {
        info!("RPC machine_list request with token");
        
        // Validate token and extract user_id
        let user_id = match self.validate_token(&token) {
            Some(uid) => uid,
            None => {
                warn!("machine_list called with invalid token");
                return vec![];
            }
        };
        
        // Get local online machines
        let local_machines = self.machines.get_online_machines(user_id).await;
        let mut machine_infos: Vec<MachineInfo> = local_machines
            .into_iter()
            .map(|m| MachineInfo {
                node_id: m.node_id.clone(),
                name: m.name,
                os: m.os,
                online: m.online,
                is_bound: true, // TODO: Track bound status
            })
            .collect();
        
        // If cluster is enabled, also get machines from other nodes
        if let Some(cluster) = &self.cluster {
            let cluster_hosts = cluster.get_online_hosts_for_user(user_id).await;
            let local_host_ids: std::collections::HashSet<_> = machine_infos.iter()
                .map(|m| m.node_id.clone())
                .collect();
            
            // Add machines from other nodes (not already in local list)
            for host_id in cluster_hosts {
                if !local_host_ids.contains(&host_id) {
                    // Get machine info from database
                    if let Ok(Some(machine)) = self.machines.db.get_machine_by_node_id(&host_id).await {
                        machine_infos.push(MachineInfo {
                            node_id: machine.node_id,
                            name: machine.name,
                            os: machine.os,
                            online: true, // It's in the cluster online set
                            is_bound: true,
                        });
                    }
                }
            }
        }
        
        info!("Returning {} online machines for user {}", machine_infos.len(), user_id);
        machine_infos
    }

    async fn machine_bind(&self, token: String, host_id: String, name: String, os: String, _current: Current) {
        info!("RPC machine_bind request: {} ({})", name, host_id);
        
        // Validate token and extract user_id
        let user_id = match self.validate_token(&token) {
            Some(uid) => uid,
            None => {
                warn!("machine_bind called with invalid token");
                return;
            }
        };
        
        // Register machine bound to this user
        match self.machines.register(&host_id, Some(user_id), &name, &os).await {
            Ok(()) => {
                if let Some(mut host_info) = self.connections.get_host_info(&host_id).await {
                    host_info.user_id = Some(user_id.to_string());
                }
                
                // Sync to cluster if enabled
                if let Some(cluster) = &self.cluster {
                    cluster.register_host_for_user(user_id, &host_id).await;
                }
                
                info!("Machine '{}' registered and bound to user {}", host_id, user_id);
            }
            Err(e) => {
                error!("Failed to register machine: {}", e);
            }
        }
    }

    async fn machine_online(&self, host_id: String, name: String, os: String, current: Current) {
        info!("RPC machine_online request: {} ({})", name, host_id);

        // Register host as online
        if let Some(connection) = current.connection() {
            info!("Connection: {:?}", connection);
            let proxy = connection.clone().create_proxy(Identity::new("HostCallback"));
            if let Ok(proxy) = HostCallbackPrx::checked_cast(proxy) {
                self.connections.register_host_online(host_id.clone(), proxy).await;
                info!("Host {} registered as online", host_id);
            }
        }

        
        // Update database - check if machine exists
        match self.machines.db.get_machine_by_node_id(&host_id).await {
            Ok(Some(_machine)) => {
                // Machine exists - update heartbeat
                if let Err(e) = self.machines.db.update_heartbeat(&host_id).await {
                    error!("Failed to update heartbeat for '{}': {}", host_id, e);
                }
            }
            Ok(None) => {
                // New machine - auto-register
                info!("New machine '{}' connected, auto-registering to database", host_id);
                if let Err(e) = self.machines.db.upsert_machine(&host_id, None, &name, &os).await {
                    error!("Failed to auto-register machine '{}': {}", host_id, e);
                }
            }
            Err(e) => {
                error!("Database error for machine '{}': {}", host_id, e);
            }
        }
    }

    async fn machine_unbind(&self, token: String, host_id: String, _current: Current) {
        info!("RPC machine_unbind request: {}", host_id);
        
        // Validate token and extract user_id
        let user_id = match self.validate_token(&token) {
            Some(uid) => uid,
            None => {
                warn!("machine_unbind called with invalid token");
                return;
            }
        };
        
        let machine_info = self.machines.db.get_machine_by_node_id(&host_id).await.ok().flatten();
        
        match self.machines.db.machine_unbind(&host_id, user_id).await {
            Ok(true) => {
                info!("Machine '{}' unbound successfully", host_id);
                
                if let Some(machine) = machine_info {
                    let change = shared::ice0_service::PeerStatusChange {
                        node_id: host_id.clone(),
                        name: machine.name.clone(),
                        os: machine.os.clone(),
                        online: false,
                    };
                    info!("Broadcasting offline status for unbound machine {} (user {:?})", host_id, machine.user_id);
                    if let Some(uid) = machine.user_id {
                        self.subscriptions.broadcast_peer_status(uid, change).await;
                    }
                }
            }
            Ok(false) => {
                warn!("Machine '{}' not found or not owned by user", host_id);
            }
            Err(e) => {
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
        current: Current,
    ) -> Answer {
        info!("RPC viewer_connect_with_offer: {} -> {} (SDP length: {})", viewer_id, host_id, offer_sdp.len());
        
        // First check if host is online locally
        if self.connections.is_online(&host_id).await {
            // Host is local - handle directly
            return self.handle_local_viewer_connection(&viewer_id, &host_id, &password, &offer_sdp).await;
        }
        
        // Host not found locally - check cluster if enabled
        if let Some(cluster) = &self.cluster {
            info!("Host {} not found locally, checking cluster...", host_id);
            
            if let Some(target_node) = cluster.lookup_host(&host_id).await {
                info!("Host {} found on node {}, forwarding request...", host_id, target_node);
                
                match cluster.forward_viewer_request(
                    &target_node,
                    &viewer_id,
                    &host_id,
                    &password,
                    &offer_sdp,
                ).await {
                    Ok(answer) => {
                        info!("Received answer from remote node for host {}", host_id);
                        answer
                    }
                    Err(e) => {
                        error!("Failed to forward viewer request to node {}: {}", target_node, e);
                        Answer {
                            accepted: false,
                            reason: format!("Cross-node forwarding failed: {}", e),
                            answer: String::new(),
                        }
                    }
                }
            } else {
                warn!("Host {} not found in cluster, rejecting viewer {}", host_id, viewer_id);
                Answer {
                    accepted: false,
                    reason: "Target host is offline".to_string(),
                    answer: String::new(),
                }
            }
        } else {
            // No cluster, host is offline
            warn!("Host {} is offline, rejecting viewer {}", host_id, viewer_id);
            Answer {
                accepted: false,
                reason: "Target host is offline".to_string(),
                answer: String::new(),
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
        _current: Current,
    ) {
        info!("RPC ice_candidate: {} -> {} (candidate length: {})", viewer_id, host_id, candidate.len());
        
        // Check if host is local
        if let Some(host_info) = self.connections.get_host_info(&host_id).await {
            let info = IceCandidateInfo {
                from: viewer_id.clone(),
                to: host_id.clone(),
                candidate: candidate.clone(),
                sdp_mid: sdp_mid.clone(),
                sdp_mline_index,
            };
            if let Err(e) = host_info.host_prx.on_ice_candidate(info).await {
                error!("Failed to forward ICE candidate to host {}: {}", host_id, e);
            }
            return;
        }
        
        // Host not local - try cluster forwarding
        if let Some(cluster) = &self.cluster {
            if let Some(target_node) = cluster.lookup_host(&host_id).await {
                info!("Forwarding ICE candidate cross-node to {} for host {}", target_node, host_id);
                if let Err(e) = cluster.forward_ice_candidate(
                    &target_node, &viewer_id, &host_id, &candidate, sdp_mid, sdp_mline_index,
                ).await {
                    error!("Failed to forward ICE candidate to node {}: {}", target_node, e);
                }
            } else {
                warn!("Host {} not found in cluster, cannot forward ICE candidate", host_id);
            }
        } else {
            warn!("Host {} is not connected locally, cannot forward ICE candidate", host_id);
        }
    }


    async fn viewer_quality_update(
        &self,
        viewer_id: String,
        quality: u32,
        max_dimension: u32,
        fps: u32,
        bitrate_kbps: u32,
        _current: Current,
    ) {
        info!(
            "RPC viewer_quality_update: {} (quality={}, dimension={}, fps={}, bitrate={})",
            viewer_id, quality, max_dimension, fps, bitrate_kbps
        );
        warn!("viewer_quality_update received but host_id not available in parameters");
    }
}
