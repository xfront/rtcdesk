//! SDP utility functions for RTCDesk.
//!
//! Encoding and extraction of custom parameters embedded in SDP.

use serde::{Deserialize, Serialize};

// ─── Capturer params via SDP ─────────────────────────────────────────

/// Host capturer configuration received from the viewer via custom SDP attributes.
#[derive(Debug, Clone, Default)]
pub struct CapturerParams {
    /// JPEG quality (0-100).
    pub pic_quality: Option<u32>,
    /// Max output dimension; frames larger than this are scaled down.
    pub max_dimension: Option<u32>,
    /// Target frame rate.
    pub fps: Option<u32>,
}

/// Encode capturer params as SDP session-level attribute lines.
/// Appends `\n`-separated `a=rtcdesk-*` attributes.
pub fn encode_capturer_params_to_sdp(params: &CapturerParams) -> String {
    let mut out = String::new();
    if let Some(q) = params.pic_quality {
        out.push_str(&format!("a=rtcdesk-quality:{}\n", q));
    }
    if let Some(d) = params.max_dimension {
        out.push_str(&format!("a=rtcdesk-max-dim:{}\n", d));
    }
    if let Some(f) = params.fps {
        out.push_str(&format!("a=rtcdesk-fps:{}\n", f));
    }
    out
}

/// Extract `CapturerParams` embedded in SDP attribute lines.
pub fn extract_capturer_params_from_sdp(sdp: &str) -> CapturerParams {
    let mut params = CapturerParams::default();
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("a=rtcdesk-quality:") {
            params.pic_quality = val.parse().ok();
        } else if let Some(val) = line.strip_prefix("a=rtcdesk-max-dim:") {
            params.max_dimension = val.parse().ok();
        } else if let Some(val) = line.strip_prefix("a=rtcdesk-fps:") {
            params.fps = val.parse().ok();
        }
    }
    params
}
