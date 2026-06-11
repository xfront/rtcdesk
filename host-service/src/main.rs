//! Host-Service - ice0 version
//!
//! Background service that:
//! - Connects to signal server via TCP (ice0)
//! - Provides IPC interface for client GUI
//! - Manages screen capture and streaming
//! - Handles WebRTC peer connections

mod capture;
mod encoder;
mod peer;
mod signal_client;
mod ipc_server;

use std::sync::Arc;
use log::{info, warn, error};
use signal_client::SignalClient;
use ipc_server::IpcHostService;
use serde::{Deserialize, Serialize};

/// Host-Service configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct HostConfig {
    pub signal_url: String,
    pub ipc_socket_path: String,
    pub username: String,
    pub password: String,
    pub host_id: String,
    pub fps: u32,
    pub stun_servers: Vec<String>,
    pub reconnect_interval_secs: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            signal_url: "ws://localhost:9009".to_string(),
            ipc_socket_path: "tcp://127.0.0.1:9010".to_string(),  // Use TCP localhost for IPC (ice0 doesn't support named pipes yet)
            username: String::new(),
            password: String::new(),
            host_id: shared::local_id::generate_local_id(),
            fps: 30,
            stun_servers: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
            ],
            reconnect_interval_secs: 5,
        }
    }
}

/// Load configuration from file
fn load_config() -> HostConfig {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rtcdesk")
        .join("rtcdesk.json");
    
    info!("Loading configuration from: {:?}", config_path);
    
    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(content) => {
                // Try to parse as HostConfig directly
                if let Ok(config) = serde_json::from_str::<HostConfig>(&content) {
                    info!("Configuration loaded successfully");
                    return config;
                }
                
                // Try to parse as client's AppConfig and extract fields
                if let Ok(app_config) = serde_json::from_str::<serde_json::Value>(&content) {
                    let mut config = HostConfig::default();
                    
                    if let Some(server_addr) = app_config.get("server_addr").and_then(|v| v.as_str()) {
                        config.signal_url = if server_addr.starts_with("ws://") || server_addr.starts_with("wss://") {
                            server_addr.to_string()
                        } else {
                            format!("ws://{}", server_addr)
                        };
                    }
                    
                    if let Some(username) = app_config.get("login_username").and_then(|v| v.as_str()) {
                        config.username = username.to_string();
                    }
                    
                    if let Some(password) = app_config.get("login_password").and_then(|v| v.as_str()) {
                        config.password = password.to_string();
                    }
                    
                    if let Some(host_password) = app_config.get("host_password").and_then(|v| v.as_str()) {
                        config.password = host_password.to_string();
                    }
                    
                    if let Some(fps) = app_config.get("fps").and_then(|v| v.as_u64()) {
                        config.fps = fps as u32;
                    }
                    
                    if let Some(stun_servers) = app_config.get("stun_servers").and_then(|v| v.as_array()) {
                        config.stun_servers = stun_servers
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                    }
                    
                    info!("Configuration loaded from client config");
                    return config;
                }
                
                warn!("Failed to parse configuration, using defaults");
            }
            Err(e) => {
                warn!("Failed to read config file: {}, using defaults", e);
            }
        }
    } else {
        info!("Config file not found, using defaults");
    }
    
    HostConfig::default()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logger
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    )
    .format_timestamp_millis()
    .init();
    
    info!("RTCDesk Host-Service starting (ice0 version)...");
    
    // Load configuration
    let config = load_config();
    
    // Start IPC server immediately (don't wait for signal connection)
    let ipc_socket_path = config.ipc_socket_path.clone();
    let host_id = config.host_id.clone();
    let fps = config.fps;
    let stun_servers = config.stun_servers.clone();
    
    // Create IPC service (it will manage SignalClient internally)
    let (peer_status_tx, _) = tokio::sync::broadcast::channel(100);
    let (ice_candidate_tx, _) = tokio::sync::broadcast::channel(100);
    
    // Create global ice0 Communicator shared between ipc_server and signal_client
    // Note: Communicator::default() returns Arc<Communicator>
    let communicator = ice0::prelude::Communicator::default().await?;
    info!("✓ Global ice0 Communicator created");
    
    let ipc_service = Arc::new(IpcHostService {
        signal_url: config.signal_url.clone(),
        reconnect_interval_secs: config.reconnect_interval_secs,
        host_id: host_id.clone(),
        username: config.username.clone(),
        password: config.password.clone(),
        signal_client: Arc::new(tokio::sync::RwLock::new(None)),
        sessions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        capturer: None, // TODO: Initialize screen capturer
        stun: stun_servers,
        fps,
        peer_status_broadcast: Arc::new(peer_status_tx),
        ice_candidate_broadcast: Arc::new(ice_candidate_tx),
        communicator, // Shared ice0 Communicator (already Arc-wrapped)
    });
    
    // Start the signal client reconnection task
    ipc_service.start_signal_client().await;
    
    info!("Starting IPC server on: {}", ipc_socket_path);
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc_service.start_ipc_server(&ipc_socket_path).await {
            error!("IPC server error: {}", e);
        }
    });
    
    info!("Host-Service is running");
    info!("Signal URL: {}", config.signal_url);
    info!("IPC socket: {}", config.ipc_socket_path);
    info!("Host ID: {}", config.host_id);
    info!("FPS: {}", config.fps);
    info!("Press Ctrl+C to stop");
    
    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");
    
    // Cleanup
    ipc_handle.abort();
    
    info!("Host-Service stopped");
    Ok(())
}
