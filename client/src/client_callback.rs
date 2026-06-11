//! ice0 ClientCallback implementation for receiving notifications from Host-Service.
//!
//! This module implements the ClientCallback trait to handle:
//! - Connection status updates
//! - Peer/machine status changes
//! - Streaming events
//! - ICE candidates from remote host (for WebRTC connection setup)

use ice0::prelude::*;
use shared::ice0_service::{ClientCallback, ConnectionStatus, PeerStatusChange, IceCandidateInfo};
use log::{info, warn};
use std::sync::Arc;
use std::sync::Mutex;

/// ClientCallback implementation for receiving callbacks from Host-Service
#[derive(Clone)]
pub struct ClientCallbackImpl {
    /// Callback for connection status changes
    connection_status_callback: Arc<Mutex<Option<Box<dyn Fn(ConnectionStatus) + Send + Sync>>>>,
    /// Callback for peer status changes
    peer_status_callback: Arc<Mutex<Option<Box<dyn Fn(PeerStatusChange) + Send + Sync>>>>,
    /// Callback for streaming events
    streaming_event_callback: Arc<Mutex<Option<Box<dyn Fn(String, String, String) + Send + Sync>>>>,
    /// Callback for ICE candidates (for WebRTC PeerConnection configuration)
    ice_candidate_callback: Arc<Mutex<Option<Box<dyn Fn(IceCandidateInfo) + Send + Sync>>>>,
}

impl ClientCallbackImpl {
    /// Create a new ClientCallbackImpl
    pub fn new() -> Self {
        Self {
            connection_status_callback: Arc::new(Mutex::new(None)),
            peer_status_callback: Arc::new(Mutex::new(None)),
            streaming_event_callback: Arc::new(Mutex::new(None)),
            ice_candidate_callback: Arc::new(Mutex::new(None)),
        }
    }

    /// Set connection status callback
    pub fn set_connection_status_callback<F>(&self, callback: F)
    where
        F: Fn(ConnectionStatus) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.connection_status_callback.lock() {
            *guard = Some(Box::new(callback));
        }
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

    /// Set streaming event callback
    pub fn set_streaming_event_callback<F>(&self, callback: F)
    where
        F: Fn(String, String, String) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.streaming_event_callback.lock() {
            *guard = Some(Box::new(callback));
        }
    }

    /// Set ICE candidate callback (for WebRTC PeerConnection configuration)
    pub fn set_ice_candidate_callback<F>(&self, callback: F)
    where
        F: Fn(IceCandidateInfo) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.ice_candidate_callback.lock() {
            *guard = Some(Box::new(callback));
        }
    }
}

#[async_trait::async_trait]
impl ClientCallback for ClientCallbackImpl {
    async fn on_peer_status_change(
        &self,
        change: PeerStatusChange,
        _ctx: Current,
    ) {
        info!("Received peer status callback: {} is now {}", 
              change.node_id, 
              if change.online { "online" } else { "offline" });
        
        if let Some(callback) = self.peer_status_callback.lock().unwrap().as_ref() {
            callback(change);
        }
    }

    async fn on_streaming_event(
        &self,
        viewer_id: String,
        event: String,
        data: String,
        _ctx: Current,
    ) {
        info!("Received streaming event callback: viewer={}, event={}, data={}", 
              viewer_id, event, data);
        
        if let Some(callback) = self.streaming_event_callback.lock().unwrap().as_ref() {
            callback(viewer_id, event, data);
        }
    }

    async fn on_ice_candidate(
        &self,
        info: IceCandidateInfo,
        _ctx: Current,
    ) {
        info!("Received ICE candidate callback from host {} to viewer {}",
              info.from, info.to);
        
        if let Some(callback) = self.ice_candidate_callback.lock().unwrap().as_ref() {
            callback(info);
        } else {
            warn!("No ICE candidate callback registered in client");
        }
    }
}
