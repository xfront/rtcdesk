//! Subscription manager for broadcasting peer status changes and viewer connection requests.
//!
//! Uses broadcast channels to push notifications
//! to all connections that belong to the same user.

use std::collections::HashMap;
use tokio::sync::{Mutex, broadcast};
use log::info;
use shared::ice0_service::{PeerStatusChange, ViewerConnectionRequest};

/// Manages subscriptions using broadcast channels
pub struct SubscriptionManager {
    /// Map from user_id to peer status broadcast sender
    peer_status_broadcasts: Mutex<HashMap<i64, broadcast::Sender<PeerStatusChange>>>,
    /// Map from host_id to viewer connection request broadcast sender
    viewer_request_broadcasts: Mutex<HashMap<String, broadcast::Sender<ViewerConnectionRequest>>>,
}

impl SubscriptionManager {
    pub fn new() -> Self {
        Self {
            peer_status_broadcasts: Mutex::new(HashMap::new()),
            viewer_request_broadcasts: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create a broadcast sender for a user, and return a receiver for peer status
    pub async fn subscribe_peer_status(&self, user_id: i64) -> broadcast::Receiver<PeerStatusChange> {
        let mut broadcasts = self.peer_status_broadcasts.lock().await;
        
        let sender = broadcasts
            .entry(user_id)
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(100);
                info!("Created peer status broadcast channel for user {}", user_id);
                tx
            });
        
        let receiver_count = sender.receiver_count();
        info!("User {} subscribed to peer status (total receivers: {})", user_id, receiver_count + 1);
        sender.subscribe()
    }

    /// Subscribe to viewer connection requests for a specific host
    pub async fn subscribe_viewer_requests(&self, host_id: String) -> broadcast::Receiver<ViewerConnectionRequest> {
        let mut broadcasts = self.viewer_request_broadcasts.lock().await;
        
        let sender = broadcasts
            .entry(host_id.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(10);
                info!("Created viewer request broadcast channel for host {}", host_id);
                tx
            });
        
        let receiver_count = sender.receiver_count();
        info!("Host {} subscribed to viewer requests (total receivers: {})", host_id, receiver_count + 1);
        sender.subscribe()
    }

    /// Broadcast a peer status change to all subscribers of the same user
    pub async fn broadcast_peer_status(&self, user_id: i64, change: PeerStatusChange) {
        let broadcasts = self.peer_status_broadcasts.lock().await;
        
        if let Some(sender) = broadcasts.get(&user_id) {
            let receiver_count = sender.receiver_count();
            info!("Broadcasting peer status change to {} receivers for user {}: {} is now {}",
                  receiver_count, user_id, change.node_id, 
                  if change.online { "online" } else { "offline" });
            
            // Send to broadcast channel (ignore error if no receivers)
            let _ = sender.send(change);
        } else {
            info!("No peer status broadcast channel for user {}", user_id);
        }
    }

    /// Broadcast a viewer connection request to the target host
    pub async fn broadcast_viewer_request(&self, host_id: &str, request: ViewerConnectionRequest) {
        let broadcasts = self.viewer_request_broadcasts.lock().await;
        
        if let Some(sender) = broadcasts.get(host_id) {
            let receiver_count = sender.receiver_count();
            info!("Broadcasting viewer request to {} receivers for host {}: viewer {}",
                  receiver_count, host_id, request.viewer_id);
            
            // Send to broadcast channel (ignore error if no receivers)
            let _ = sender.send(request);
        } else {
            info!("No viewer request broadcast channel for host {}", host_id);
        }
    }
}
