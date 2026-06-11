//! WebRTC connection flow using ice0 RPC.
//!
//! Implements the complete Offer/Answer/ICE exchange via RPC:
//! 1. Viewer creates Offer SDP
//! 2. Sends Offer to host via RPC
//! 3. Receives Answer SDP from host
//! 4. Sets Answer on PeerConnection
//! 5. Exchanges ICE candidates (bundled in SDP for now)

use anyhow::{Context, Result};
use log::{error, info};
use std::sync::Arc;
use crate::rpc_client::RpcClient;
use crate::viewer::{RtcViewer, ViewerCallback};

/// Establish WebRTC connection to remote host via ice0 RPC
///
/// # Arguments
/// * `viewer` - The RtcViewer instance
/// * `rpc_client` - RPC client connected to host-service
/// * `host_id` - Remote host ID to connect to
/// * `password` - Connection password
/// * `stun_servers` - List of STUN server URLs
/// * `callback` - Viewer callback for events
///
/// # Returns
/// The connected RtcViewer instance
pub async fn connect_via_rpc(
    mut viewer: RtcViewer,
    rpc_client: Arc<RpcClient>,
    host_id: String,
    password: String,
    stun_servers: Vec<String>,
    callback: Arc<dyn ViewerCallback>,
) -> Result<RtcViewer> {
    info!("Starting WebRTC connection to host '{}' via RPC", host_id);

    // Step 1: Create Offer SDP
    let offer_sdp = viewer.create_offer(stun_servers.clone()).await
        .context("failed to create offer")?;
    
    info!("Created Offer SDP (length: {} bytes)", offer_sdp.len());

    // Step 2: Send Offer to host via RPC and get Answer
    let viewer_id = viewer.viewer_id().to_string();
    let answer = rpc_client.viewer_connect_with_offer(
        viewer_id.clone(),
        host_id.clone(),
        password,
        offer_sdp,
    ).await
        .map_err(|e| anyhow::anyhow!("RPC call failed: {}", e))?;

    if !answer.accepted {
        anyhow::bail!("Connection rejected by host: {}", answer.reason);
    }

    info!("Received Answer SDP from host (length: {} bytes)", answer.answer.len());

    // Step 3: Set Answer SDP on PeerConnection
    viewer.set_answer(answer.answer).await
        .context("failed to set answer")?;

    info!("WebRTC connection established to host '{}'", host_id);

    // Step 4: ICE candidates are bundled in the SDP (bundled ICE approach)
    // For trickle ICE, we would set up ICE candidate forwarding here

    Ok(viewer)
}
