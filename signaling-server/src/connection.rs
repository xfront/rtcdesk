//! Connection manager for tracking ice0 connections.
//!
//! Each machine has exactly one ice0 connection.
//! Tracks online hosts and their callback clients for bidirectional RPC.

use std::collections::HashMap;
use std::sync::Arc;
use std::hash::{Hash, Hasher};
use tokio::sync::{RwLock, Mutex};
use log::{info, warn};
use ice0::current::Current;
use ice0::ObjectPrx;
use shared::ice0_service::HostCallbackPrx;

use crate::cluster::ClusterManager;

/// Callback type for connection disconnect events
pub type DisconnectCallback = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct HostInfo {
    pub user_id: Option<String>,
    pub access_key: Option<String>,
    pub host_prx: HostCallbackPrx,
}
/// Manages active ice0 connections
/// Each machine (identified by host_id) has exactly one connection
pub struct ConnectionManager {
    /// Callback invoked when a connection is removed
    disconnect_callback: RwLock<Option<DisconnectCallback>>,
    /// Current context for each connected host (allows reverse proxy to HostCallback)
    host_contexts: Arc<Mutex<HashMap<String, HostInfo>>>,
    /// Optional cluster manager for multi-node deployment
    cluster: Option<Arc<ClusterManager>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            disconnect_callback: RwLock::new(None),
            host_contexts: Arc::new(Mutex::new(HashMap::new())),
            cluster: None,
        }
    }

    /// Set the cluster manager for multi-node support
    pub fn set_cluster(&mut self, cluster: Arc<ClusterManager>) {
        self.cluster = Some(cluster);
    }

    /// Set the disconnect callback
    pub async fn set_disconnect_callback<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        *self.disconnect_callback.write().await = Some(Arc::new(callback));
    }

    /// Register a machine as online (with Current context from RPC call)
    pub async fn register_host_online(&self, host_id: String, proxy: HostCallbackPrx) {
        let mut contexts = self.host_contexts.lock().await;
        
        if contexts.contains_key(&host_id) {
            warn!("Machine {} was already registered as online", host_id);
        } else {
            contexts.insert(host_id.clone(), HostInfo {
                user_id: None,
                access_key: None,
                host_prx: proxy,
            });
            info!("Machine {} registered as online (total: {})", host_id, contexts.len());
        }
        
        // Sync to cluster if enabled
        if let Some(cluster) = &self.cluster {
            cluster.register_host(&host_id).await;
        }
    }

    /// Remove a machine when it disconnects
    pub async fn unregister_host(&self, host_id: &str) {
        let mut contexts = self.host_contexts.lock().await;
        
        if contexts.remove(host_id).is_some() {
            info!("Machine {} unregistered (total: {})", host_id, contexts.len());
            
            // Sync to cluster if enabled
            if let Some(cluster) = &self.cluster {
                cluster.unregister_host(host_id).await;
            }
            
            // Invoke disconnect callback
            if let Some(callback) = self.disconnect_callback.read().await.as_ref() {
                callback(host_id.to_string());
            }
        } else {
            warn!("Machine {} was not registered", host_id);
        }
    }

    /// Check if a machine is online (has an active connection)
    pub async fn is_online(&self, host_id: &str) -> bool {
        self.host_contexts.lock().await.contains_key(host_id)
    }

    /// Get count of online machines
    pub async fn online_count(&self) -> usize {
        self.host_contexts.lock().await.len()
    }

    /// Get online machine IDs for a specific user (from active RPC connections)
    pub async fn get_active_machines(&self, user_id: Option<&str>) -> Vec<String> {
        let contexts = self.host_contexts.lock().await;
        
        if let Some(uid) = user_id {
            // Filter machines by user_id
            contexts
                .iter()
                .filter(|(_, info)| info.user_id.as_deref() == Some(uid))
                .map(|(host_id, _)| host_id.clone())
                .collect()
        } else {
            // Return all online machines if no user_id specified
            contexts.keys().cloned().collect()
        }
    }

    /// Get HostInfo for a host
    pub async fn get_host_info(&self, host_id: &str) -> Option<HostInfo> {
        let contexts = self.host_contexts.lock().await;
        contexts.get(host_id).cloned()
    }

    /// Update HostInfo with user_id and access_key
    pub async fn update_host_info(&self, host_id: &str, user_id: String, access_key: String) {
        let mut contexts = self.host_contexts.lock().await;
        if let Some(info) = contexts.get_mut(host_id) {
            info.user_id = Some(user_id);
            info.access_key = Some(access_key);
            info!("Updated host {} with user_id and access_key", host_id);
        }
    }

    /// Generate a pseudo connection_id from host_id hash
    /// Useful for legacy APIs that require connection_id
    pub fn host_id_to_pseudo_id(host_id: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        host_id.hash(&mut hasher);
        hasher.finish()
    }
}
