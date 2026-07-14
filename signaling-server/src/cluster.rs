//! Cluster support for multi-node signal server deployment.
//!
//! Uses Redis for:
//! - Node registry and health monitoring
//! - Host-to-node mapping (which host is connected to which node)
//! - Cross-node request/response via pub/sub
//! - Cluster-wide peer status broadcasts

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
use log::{info, warn, error};
use serde::{Serialize, Deserialize};
use futures::StreamExt;
use shared::ice0_service::{Answer, PeerStatusChange};

// Redis key prefixes
const CLUSTER_NODES_KEY: &str = "cluster:nodes";
const CLUSTER_HOST_PREFIX: &str = "cluster:host:";
const CLUSTER_USER_PREFIX: &str = "cluster:user:";
const NODE_REQUEST_CHANNEL_PREFIX: &str = "node:";
const NODE_RESPONSE_CHANNEL_SUFFIX: &str = ":responses";
const NODE_REQUEST_CHANNEL_SUFFIX: &str = ":requests";
const BROADCAST_CHANNEL: &str = "cluster:broadcast";

// ─── Callback types for handling incoming cross-node messages ───────────────

pub type ViewerRequestCallback = Arc<dyn Fn(String, String, String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>> + Send + Sync>;
pub type IceCandidateCallback = Arc<dyn Fn(String, String, String, Option<String>, Option<u16>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;
pub type PeerStatusCallback = Arc<dyn Fn(PeerStatusChange) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Holds callbacks for handling incoming cross-node messages
pub struct MessageHandler {
    pub on_viewer_request: ViewerRequestCallback,
    pub on_ice_candidate: IceCandidateCallback,
    pub on_peer_status: PeerStatusCallback,
}

/// Host-to-node mapping TTL (refreshed by heartbeat)
const HOST_MAPPING_TTL_SECS: u64 = 120;
/// Node heartbeat interval
const NODE_HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// Cross-node request timeout
const CROSS_NODE_TIMEOUT_SECS: u64 = 10;

// ─── Cross-Node Messages ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrossNodeMessage {
    ViewerConnectRequest {
        request_id: String,
        from_node: String,
        viewer_id: String,
        host_id: String,
        password: String,
        offer_sdp: String,
    },
    ViewerConnectResponse {
        request_id: String,
        from_node: String,
        answer: Answer,
    },
    PeerStatusBroadcast(PeerStatusChange),
    /// ICE candidate forwarded across nodes (oneway)
    IceCandidateForward {
        viewer_id: String,
        host_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

// ─── NodeRegistry ────────────────────────────────────────────────────────────

/// Manages node registration and host-to-node mapping in Redis
pub struct NodeRegistry {
    redis: redis::aio::MultiplexedConnection,
    node_id: String,
    node_addr: String,
}

impl NodeRegistry {
    pub fn new(redis: redis::aio::MultiplexedConnection, node_id: String, node_addr: String) -> Self {
        Self { redis, node_id, node_addr }
    }

    /// Register this node in the cluster
    pub async fn register_node(&mut self) -> anyhow::Result<()> {
        let node_info = serde_json::json!({
            "addr": self.node_addr,
            "last_seen": chrono::Utc::now().timestamp()
        }).to_string();

        let _: () = redis::cmd("HSET")
            .arg(CLUSTER_NODES_KEY)
            .arg(&self.node_id)
            .arg(&node_info)
            .query_async(&mut self.redis)
            .await?;

        info!("Node '{}' registered in cluster (addr: {})", self.node_id, self.node_addr);
        Ok(())
    }

    /// Update node heartbeat
    pub async fn heartbeat(&mut self) -> anyhow::Result<()> {
        let node_info = serde_json::json!({
            "addr": self.node_addr,
            "last_seen": chrono::Utc::now().timestamp()
        }).to_string();

        let _: () = redis::cmd("HSET")
            .arg(CLUSTER_NODES_KEY)
            .arg(&self.node_id)
            .arg(&node_info)
            .query_async(&mut self.redis)
            .await?;
        Ok(())
    }

    /// Unregister this node from the cluster
    pub async fn unregister_node(&mut self) -> anyhow::Result<()> {
        let _: () = redis::cmd("HDEL")
            .arg(CLUSTER_NODES_KEY)
            .arg(&self.node_id)
            .query_async(&mut self.redis)
            .await?;
        info!("Node '{}' unregistered from cluster", self.node_id);
        Ok(())
    }

    /// Register a host as being on this node
    pub async fn register_host(&mut self, host_id: &str) -> anyhow::Result<()> {
        let key = format!("{}{}", CLUSTER_HOST_PREFIX, host_id);
        let _: () = redis::cmd("SETEX")
            .arg(&key)
            .arg(HOST_MAPPING_TTL_SECS)
            .arg(&self.node_id)
            .query_async(&mut self.redis)
            .await?;
        Ok(())
    }

    /// Remove a host mapping
    pub async fn unregister_host(&mut self, host_id: &str) -> anyhow::Result<()> {
        let key = format!("{}{}", CLUSTER_HOST_PREFIX, host_id);
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut self.redis)
            .await?;
        Ok(())
    }

    /// Look up which node a host is connected to
    pub async fn lookup_host(&mut self, host_id: &str) -> Option<String> {
        let key = format!("{}{}", CLUSTER_HOST_PREFIX, host_id);
        let result: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut self.redis)
            .await
            .ok()?;
        result
    }

    /// Get all online hosts for a user across the cluster
    pub async fn get_online_hosts_for_user(&mut self, user_id: i64) -> Vec<String> {
        let key = format!("{}{}", CLUSTER_USER_PREFIX, user_id);
        let result: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&key)
            .query_async(&mut self.redis)
            .await
            .unwrap_or_default();
        result
    }

    /// Add a host to a user's online set
    pub async fn add_host_to_user(&mut self, user_id: i64, host_id: &str) -> anyhow::Result<()> {
        let key = format!("{}{}", CLUSTER_USER_PREFIX, user_id);
        let _: () = redis::cmd("SADD")
            .arg(&key)
            .arg(host_id)
            .query_async(&mut self.redis)
            .await?;
        Ok(())
    }

    /// Remove a host from a user's online set
    pub async fn remove_host_from_user(&mut self, user_id: i64, host_id: &str) -> anyhow::Result<()> {
        let key = format!("{}{}", CLUSTER_USER_PREFIX, user_id);
        let _: () = redis::cmd("SREM")
            .arg(&key)
            .arg(host_id)
            .query_async(&mut self.redis)
            .await?;
        Ok(())
    }

    /// Get all known node IDs
    pub async fn get_all_nodes(&mut self) -> Vec<String> {
        let result: Vec<String> = redis::cmd("HKEYS")
            .arg(CLUSTER_NODES_KEY)
            .query_async(&mut self.redis)
            .await
            .unwrap_or_default();
        result
    }
}

// ─── CrossNodeRouter ─────────────────────────────────────────────────────────

/// Handles cross-node request/response via Redis pub/sub
pub struct CrossNodeRouter {
    redis: redis::aio::MultiplexedConnection,
    node_id: String,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<CrossNodeMessage>>>>,
}

impl CrossNodeRouter {
    pub fn new(redis: redis::aio::MultiplexedConnection, node_id: String) -> Self {
        Self {
            redis,
            node_id,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the response channel for this node
    fn response_channel(&self) -> String {
        format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, self.node_id, NODE_RESPONSE_CHANNEL_SUFFIX)
    }

    /// Get the request channel for a target node
    fn request_channel(target_node: &str) -> String {
        format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, target_node, NODE_REQUEST_CHANNEL_SUFFIX)
    }

    /// Send a cross-node request and wait for response
    pub async fn send_request(
        &self,
        target_node: &str,
        message: CrossNodeMessage,
    ) -> anyhow::Result<CrossNodeMessage> {
        // Extract request_id from the message
        let request_id = match &message {
            CrossNodeMessage::ViewerConnectRequest { request_id, .. } => request_id.clone(),
            _ => anyhow::bail!("send_request only supports request messages"),
        };

        // Create oneshot channel for response correlation
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        // Serialize and publish to target node's request channel
        let channel = Self::request_channel(target_node);
        let payload = serde_json::to_string(&message)?;

        let mut conn = self.redis.clone();
        let _: () = redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(&payload)
            .query_async(&mut conn)
            .await?;

        info!("Sent cross-node request {} to node {}", request_id, target_node);

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(CROSS_NODE_TIMEOUT_SECS), rx).await {
            Ok(Ok(response)) => {
                info!("Received cross-node response for request {}", request_id);
                Ok(response)
            }
            Ok(Err(_)) => {
                anyhow::bail!("Cross-node request {} channel closed", request_id);
            }
            Err(_) => {
                // Clean up pending entry on timeout
                let mut pending = self.pending.lock().await;
                pending.remove(&request_id);
                anyhow::bail!("Cross-node request {} timed out after {}s", request_id, CROSS_NODE_TIMEOUT_SECS);
            }
        }
    }

    /// Publish a response back to the requesting node
    pub async fn publish_response(
        &self,
        target_node: &str,
        message: CrossNodeMessage,
    ) -> anyhow::Result<()> {
        let channel = format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, target_node, NODE_RESPONSE_CHANNEL_SUFFIX);
        let payload = serde_json::to_string(&message)?;

        let mut conn = self.redis.clone();
        let _: () = redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(&payload)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    /// Broadcast peer status change to all nodes
    pub async fn broadcast_peer_status(&self, change: PeerStatusChange) -> anyhow::Result<()> {
        let message = CrossNodeMessage::PeerStatusBroadcast(change);
        let payload = serde_json::to_string(&message)?;

        let mut conn = self.redis.clone();
        let _: () = redis::cmd("PUBLISH")
            .arg(BROADCAST_CHANNEL)
            .arg(&payload)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    /// Get the pending requests map (for the pub/sub listener to resolve responses)
    pub fn pending_requests(&self) -> Arc<Mutex<HashMap<String, oneshot::Sender<CrossNodeMessage>>>> {
        self.pending.clone()
    }

    /// Publish a raw payload to an arbitrary Redis channel
    pub async fn publish_to_channel(&self, channel: &str, payload: &str) -> anyhow::Result<()> {
        let mut conn = self.redis.clone();
        let _: () = redis::cmd("PUBLISH")
            .arg(channel)
            .arg(payload)
            .query_async(&mut conn)
            .await?;
        Ok(())
    }
}

// ─── ClusterManager ──────────────────────────────────────────────────────────

/// Top-level cluster orchestrator
pub struct ClusterManager {
    pub node_id: String,
    registry: Arc<Mutex<NodeRegistry>>,
    router: Arc<CrossNodeRouter>,
    shutdown_tx: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
    /// Callbacks for handling incoming cross-node messages (set via set_handler)
    handler: Arc<tokio::sync::RwLock<Option<MessageHandler>>>,
    /// Redis URL for creating a dedicated pub/sub connection
    redis_url: String,
}

impl ClusterManager {
    /// Create a new ClusterManager (does not start background tasks yet)
    pub async fn new(redis_url: &str, node_id: String, node_addr: String) -> anyhow::Result<Self> {
        let client = redis::Client::open(redis_url)?;
        let redis = client.get_multiplexed_tokio_connection().await?;

        let registry = NodeRegistry::new(redis.clone(), node_id.clone(), node_addr);
        let router = CrossNodeRouter::new(redis, node_id.clone());

        Ok(Self {
            node_id,
            registry: Arc::new(Mutex::new(registry)),
            router: Arc::new(router),
            shutdown_tx: Arc::new(Mutex::new(None)),
            handler: Arc::new(tokio::sync::RwLock::new(None)),
            redis_url: redis_url.to_string(),
        })
    }

    /// Set the message handler callbacks for processing incoming cross-node messages.
    /// Must be called before `start_listener()`.
    pub async fn set_handler(&self, handler: MessageHandler) {
        *self.handler.write().await = Some(handler);
        info!("Cluster message handler registered for node '{}'", self.node_id);
    }

    /// Start background tasks: node heartbeat, pub/sub listener, health check
    pub async fn start(&self) -> anyhow::Result<()> {
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        *self.shutdown_tx.lock().await = Some(shutdown_tx.clone());

        // Register this node
        {
            let mut registry = self.registry.lock().await;
            registry.register_node().await?;
        }

        // Start node heartbeat task
        let registry_clone = self.registry.clone();
        let node_id = self.node_id.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(NODE_HEARTBEAT_INTERVAL_SECS));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let mut registry = registry_clone.lock().await;
                        if let Err(e) = registry.heartbeat().await {
                            warn!("Node {} heartbeat failed: {}", node_id, e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Node {} heartbeat task shutting down", node_id);
                        break;
                    }
                }
            }
        });

        info!("Cluster manager started for node '{}' (listener not yet started)", self.node_id);
        Ok(())
    }

    /// Start the Redis pub/sub listener for incoming cross-node messages.
    /// Call this AFTER `set_handler()` so callbacks are ready.
    pub async fn start_listener(&self) -> anyhow::Result<()> {
        let client = redis::Client::open(self.redis_url.as_str())?;
        let mut pubsub_conn = client.get_tokio_connection().await?.into_pubsub();

        let request_channel = format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, self.node_id, NODE_REQUEST_CHANNEL_SUFFIX);
        let response_channel = format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, self.node_id, NODE_RESPONSE_CHANNEL_SUFFIX);

        pubsub_conn.subscribe(&[&request_channel, &response_channel, BROADCAST_CHANNEL]).await?;
        info!("Cluster pub/sub listener started for node '{}' (channels: {}, {}, {})",
              self.node_id, request_channel, response_channel, BROADCAST_CHANNEL);

        let handler = self.handler.clone();
        let pending = self.router.pending_requests().clone();
        let router = self.router.clone();
        let node_id = self.node_id.clone();
        let mut shutdown_rx = self.shutdown_tx.lock().await.as_ref().unwrap().subscribe();

        tokio::spawn(async move {
            let mut msg_stream = pubsub_conn.on_message();
            loop {
                tokio::select! {
                    msg = msg_stream.next() => {
                        let msg = match msg {
                            Some(m) => m,
                            None => continue,
                        };
                        let channel: String = match msg.get_channel() {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        let payload: String = match msg.get_payload() {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let message: CrossNodeMessage = match serde_json::from_str(&payload) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("Failed to deserialize cross-node message: {}", e);
                                continue;
                            }
                        };

                        Self::handle_message(
                            &node_id, &channel, message, &handler, &pending, &router,
                        ).await;
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Node '{}' pub/sub listener shutting down", node_id);
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Process an incoming cross-node message. Spawns tasks for blocking operations.
    async fn handle_message(
        node_id: &str,
        _channel: &str,
        message: CrossNodeMessage,
        handler: &Arc<tokio::sync::RwLock<Option<MessageHandler>>>,
        pending: &Arc<Mutex<HashMap<String, oneshot::Sender<CrossNodeMessage>>>>,
        router: &Arc<CrossNodeRouter>,
    ) {
        match message {
            CrossNodeMessage::ViewerConnectRequest {
                request_id, from_node, viewer_id, host_id, password, offer_sdp,
            } => {
                info!("[{}] Received viewer connect request {} from node {} for host {}",
                      node_id, request_id, from_node, host_id);
                let handler = handler.clone();
                let router = router.clone();
                let from_node_clone = from_node.clone();
                let request_id_clone = request_id.clone();
                let nid = node_id.to_string();
                tokio::spawn(async move {
                    let h = handler.read().await;
                    let answer = if let Some(ref h) = *h {
                        (h.on_viewer_request)(viewer_id, host_id, password, offer_sdp).await
                    } else {
                        warn!("[{}] No handler registered, rejecting cross-node request", nid);
                        Answer {
                            accepted: false,
                            reason: "Node not ready".to_string(),
                            answer: String::new(),
                        }
                    };
                    let response = CrossNodeMessage::ViewerConnectResponse {
                        request_id: request_id_clone.clone(),
                        from_node: nid.clone(),
                        answer,
                    };
                    if let Err(e) = router.publish_response(&from_node_clone, response).await {
                        error!("[{}] Failed to send response for request {}: {}", nid, request_id_clone, e);
                    }
                });
            }
            CrossNodeMessage::IceCandidateForward {
                viewer_id, host_id, candidate, sdp_mid, sdp_mline_index,
            } => {
                info!("[{}] Received ICE candidate for host {} from viewer {}", node_id, host_id, viewer_id);
                let handler = handler.clone();
                let h = handler.read().await;
                if let Some(ref h) = *h {
                    (h.on_ice_candidate)(viewer_id, host_id, candidate, sdp_mid, sdp_mline_index).await;
                } else {
                    warn!("[{}] No handler registered, ICE candidate ignored", node_id);
                }
            }
            CrossNodeMessage::PeerStatusBroadcast(change) => {
                info!("[{}] Received peer status broadcast: {} is {}",
                      node_id, change.node_id, if change.online { "online" } else { "offline" });
                let handler = handler.clone();
                let h = handler.read().await;
                if let Some(ref h) = *h {
                    (h.on_peer_status)(change).await;
                } else {
                    warn!("[{}] No handler registered, peer status broadcast ignored", node_id);
                }
            }
            CrossNodeMessage::ViewerConnectResponse {
                request_id, from_node, answer,
            } => {
                info!("[{}] Received cross-node response for request {} from node {}",
                      node_id, request_id, from_node);
                let mut pending = pending.lock().await;
                if let Some(sender) = pending.remove(&request_id) {
                    let _ = sender.send(CrossNodeMessage::ViewerConnectResponse {
                        request_id,
                        from_node,
                        answer,
                    });
                } else {
                    warn!("No pending request for response {} (timed out?)", request_id);
                }
            }
        }
    }

    /// Stop the cluster manager and cleanup
    pub async fn stop(&self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        let mut registry = self.registry.lock().await;
        registry.unregister_node().await?;
        info!("Cluster manager stopped for node '{}'", self.node_id);
        Ok(())
    }

    /// Register a host as online in the cluster
    pub async fn register_host(&self, host_id: &str) {
        let mut registry = self.registry.lock().await;
        if let Err(e) = registry.register_host(host_id).await {
            error!("Failed to register host {} in cluster: {}", host_id, e);
        }
    }

    /// Unregister a host from the cluster
    pub async fn unregister_host(&self, host_id: &str) {
        let mut registry = self.registry.lock().await;
        if let Err(e) = registry.unregister_host(host_id).await {
            error!("Failed to unregister host {} from cluster: {}", host_id, e);
        }
    }

    /// Register a host under a user's online set
    pub async fn register_host_for_user(&self, user_id: i64, host_id: &str) {
        let mut registry = self.registry.lock().await;
        if let Err(e) = registry.add_host_to_user(user_id, host_id).await {
            error!("Failed to add host {} to user {} in cluster: {}", host_id, user_id, e);
        }
    }

    /// Remove a host from a user's online set
    pub async fn unregister_host_for_user(&self, user_id: i64, host_id: &str) {
        let mut registry = self.registry.lock().await;
        if let Err(e) = registry.remove_host_from_user(user_id, host_id).await {
            error!("Failed to remove host {} from user {} in cluster: {}", host_id, user_id, e);
        }
    }

    /// Look up which node a host is on (cluster-wide)
    pub async fn lookup_host(&self, host_id: &str) -> Option<String> {
        let mut registry = self.registry.lock().await;
        registry.lookup_host(host_id).await
    }

    /// Get all online hosts for a user across the cluster
    pub async fn get_online_hosts_for_user(&self, user_id: i64) -> Vec<String> {
        let mut registry = self.registry.lock().await;
        registry.get_online_hosts_for_user(user_id).await
    }

    /// Forward a viewer connection request to the node where the host is connected
    pub async fn forward_viewer_request(
        &self,
        target_node: &str,
        viewer_id: &str,
        host_id: &str,
        password: &str,
        offer_sdp: &str,
    ) -> anyhow::Result<Answer> {
        let request_id = uuid::Uuid::new_v4().to_string();

        let message = CrossNodeMessage::ViewerConnectRequest {
            request_id: request_id.clone(),
            from_node: self.node_id.clone(),
            viewer_id: viewer_id.to_string(),
            host_id: host_id.to_string(),
            password: password.to_string(),
            offer_sdp: offer_sdp.to_string(),
        };

        let response = self.router.send_request(target_node, message).await?;

        match response {
            CrossNodeMessage::ViewerConnectResponse { answer, .. } => Ok(answer),
            _ => anyhow::bail!("Unexpected response type for request {}", request_id),
        }
    }

    /// Broadcast peer status change to all nodes
    pub async fn broadcast_peer_status(&self, change: PeerStatusChange) {
        if let Err(e) = self.router.broadcast_peer_status(change).await {
            warn!("Failed to broadcast peer status: {}", e);
        }
    }

    /// Forward an ICE candidate to the node where the host is connected (oneway)
    pub async fn forward_ice_candidate(
        &self,
        target_node: &str,
        viewer_id: &str,
        host_id: &str,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> anyhow::Result<()> {
        let message = CrossNodeMessage::IceCandidateForward {
            viewer_id: viewer_id.to_string(),
            host_id: host_id.to_string(),
            candidate: candidate.to_string(),
            sdp_mid,
            sdp_mline_index,
        };

        let channel = format!("{}{}{}", NODE_REQUEST_CHANNEL_PREFIX, target_node, NODE_REQUEST_CHANNEL_SUFFIX);
        let payload = serde_json::to_string(&message)?;

        self.router.publish_to_channel(&channel, &payload).await?;

        info!("Forwarded ICE candidate to node {} for host {}", target_node, host_id);
        Ok(())
    }
}
