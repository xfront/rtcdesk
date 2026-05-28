//! RTCDesk Console Client
//!
//! Standalone console application that connects to a signaling server
//! as a viewer and prints all sent/received data.
//!
//! ## Usage
//!
//! ```bash
//! cargo run --bin client -- [-s ADDR] [-v VIEWER_ID] [-h HOST_ID]
//! ```
//!
//! Flags:
//! - `-s ADDR`       — signaling server address (env: RTCDESK_ADDR, default: 127.0.0.1:9009)
//! - `-v VIEWER_ID`  — this viewer's ID       (env: RTCDESK_VIEWER_ID, default: console-viewer)
//! - `-h HOST_ID`    — target host ID         (env: RTCDESK_HOST_ID, default: desktop-host)
//! - `--stun URLS`   — comma-separated STUN   (env: RTCDESK_STUN, default: "" = LAN-only)
//!
//! Priority: CLI flags > environment variables > defaults

mod frame;
mod signal;
mod viewer;

use anyhow::{Context, Result};
use log::{error, info};
use std::sync::Arc;
use viewer::RtcViewer;

/// Resolve a config value: env var → default.
fn resolve_arg(env_key: &str, default: &str) -> String {
    std::env::var(env_key).unwrap_or_else(|_| default.to_string())
}

// ─── Console Callback ────────────────────────────────────────────────

struct ConsoleCallback;

impl viewer::ViewerCallback for ConsoleCallback {
    fn on_connected(&self) {
        info!("[Console] ✅ Connected to host — receiving desktop stream");
    }

    fn on_disconnected(&self) {
        info!("[Console] ❌ Disconnected from host");
    }

    fn on_frame(&self, data: Vec<u8>, width: u32, height: u32) {
        // Print frame summary (don't flood with every frame)
        // Use warn level so it always shows even with default info filter
        info!(
            "[Console] 📷 Frame: {}×{} ({} bytes, {} KB)",
            width,
            height,
            data.len(),
            data.len() / 1024
        );
    }

    fn on_connection_state(&self, state: String) {
        info!("[Console] 🔗 Connection state: {}", state);
    }

    fn on_error(&self, message: String) {
        error!("[Console] ⚠️ Error: {}", message);
    }

    fn on_clipboard_text(&self, text: String) {
        info!(
            "[Console] 📋 Clipboard text from host ({} bytes)",
            text.len()
        );
    }

    fn on_clipboard_file_uris(&self, uris: String) {
        info!("[Console] 📁 Clipboard files from host: {}", uris);
    }
}

// ─── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 requires explicit crypto provider initialization
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Parse CLI flags: -s ADDR  -v VIEWER_ID  -h HOST_ID  --stun URLS
    let args: Vec<String> = std::env::args().collect();
    let mut server_addr = None;
    let mut viewer_id = None;
    let mut host_id = None;
    let mut stun = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-s" if i + 1 < args.len() => {
                server_addr = Some(args[i + 1].clone());
                i += 2;
            }
            "-v" if i + 1 < args.len() => {
                viewer_id = Some(args[i + 1].clone());
                i += 2;
            }
            "-h" if i + 1 < args.len() => {
                host_id = Some(args[i + 1].clone());
                i += 2;
            }
            "--stun" if i + 1 < args.len() => {
                stun = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let server_addr = server_addr.unwrap_or_else(|| resolve_arg("RTCDESK_ADDR", "127.0.0.1:9009"));
    let viewer_id = viewer_id.unwrap_or_else(|| resolve_arg("RTCDESK_VIEWER_ID", "console-viewer"));
    let host_id = host_id.unwrap_or_else(|| resolve_arg("RTCDESK_HOST_ID", "desktop-host"));
    let stun_raw = stun.unwrap_or_else(|| resolve_arg("RTCDESK_STUN", ""));
    let stun_servers: Vec<String> = stun_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!("══════════════════════════════════════");
    info!("  RTCDesk Console Client");
    info!("══════════════════════════════════════");
    info!("  Server:    {}", server_addr);
    info!("  Viewer ID: {}", viewer_id);
    info!("  Host ID:   {}", host_id);
    info!("══════════════════════════════════════");

    let mut viewer = RtcViewer::new(viewer_id.clone(), host_id.clone());
    viewer.set_callback(Arc::new(ConsoleCallback));
    viewer.set_pic_quality(40);

    info!("Connecting to signaling server...");
    viewer
        .connect(server_addr, stun_servers)
        .await
        .context("Failed to connect to server")?;

    info!("Connected! Waiting for stream data...");
    info!("Press Ctrl+C to exit.");

    // Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .context("Failed to listen for Ctrl+C")?;

    info!("Shutting down...");
    viewer.disconnect().await;
    info!("Done. Goodbye!");

    Ok(())
}
