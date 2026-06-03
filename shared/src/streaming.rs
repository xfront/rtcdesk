//! Streaming video protocol definitions for codec-compressed frames
//! over DataChannel.
//!
//! ## Wire format
//!
//! Each video frame is delimited with a 4-byte big-endian length prefix,
//! followed by a format byte, a compact frame header, and the payload:
//!
//! ```text
//! ┌────────────┬─────────┬──────────────┬──────────────┬──────────────┬──────────────┐
//! │ frame_len  │ fmt u8  │ orig_w  u16  │ orig_h  u16  │ width   u16  │ height  u16  │
//! │  u32 BE    │         │     BE       │     BE       │     BE       │     BE       │
//! └────────────┴─────────┴──────────────┴──────────────┴──────────────┴──────────────┘
//!   0…3         4         5…6            7…8            9…10           11…12
//! → followed by frame data (frame_len - 9 bytes)
//! ```
//!
//! Total header: 13 bytes (4 length + 1 format + 8 resolution).
//!
//! ## Frame format identifiers
//!
//! | Value | Constant          | Description                       |
//! |-------|-------------------|-----------------------------------|
//! | 1     | FRAME_FMT_RGBA    | Raw RGBA pixel data               |
//! | 2     | FRAME_FMT_H264    | H.264 / AVC Annex B byte stream   |
//! | 3     | FRAME_FMT_H265    | H.265 / HEVC Annex B byte stream  |
//! | 4     | FRAME_FMT_AV1     | AV1 IVF container                 |
//! | 5     | FRAME_FMT_VP8     | VP8 IVF container                 |
//! | 6     | FRAME_FMT_VP9     | VP9 IVF container                 |
//!
//! ## Stream control messages (sent over the same data channel)
//!
//! The host can also send control frames (length = 0) to signal events:
//! - `STREAM_CTRL_RESYNC` — force client to reset decoder state
//!
//! ## Adaptive feedback
//!
//! The client sends buffered_amount / throughput feedback via the signaling
//! channel (`AdaptiveFeedback` message) so the host can adjust fps or
//! resolution on the fly — beyond the coarse `QualityUpdate` signaling.

/// 4-byte big-endian length of the *payload* (header + data, excluding this prefix).
pub const STREAM_FRAME_LEN_SIZE: usize = 4;

/// Compact stream frame header: fmt(1) + orig_w(2) + orig_h(2) + w(2) + h(2).
pub const STREAM_FRAME_HDR_SIZE: usize = 9;

/// Total prefix per frame: length + header.
pub const STREAM_FRAME_PREFIX: usize = STREAM_FRAME_LEN_SIZE + STREAM_FRAME_HDR_SIZE;

// ── Frame format identifiers ──────────────────────────────────────────

/// Raw RGBA pixel data (width × height × 4 bytes).
pub const FRAME_FMT_RGBA: u8 = 1;
/// H.264 / AVC Annex B byte stream.
pub const FRAME_FMT_H264: u8 = 2;
/// H.265 / HEVC Annex B byte stream.
pub const FRAME_FMT_H265: u8 = 3;
/// AV1 bitstream in IVF container.
pub const FRAME_FMT_AV1: u8 = 4;
/// VP8 bitstream in IVF container.
pub const FRAME_FMT_VP8: u8 = 5;
/// VP9 bitstream in IVF container.
pub const FRAME_FMT_VP9: u8 = 6;

/// Max encoded payload size (accommodates compressed keyframes).
pub const MAX_ENCODED_PAYLOAD: usize = 16 * 1024 * 1024; // 16 MB

/// Return a human-readable name for a frame format byte.
pub const fn codec_label(fmt: u8) -> &'static str {
    match fmt {
        FRAME_FMT_RGBA => "RGBA",
        FRAME_FMT_H264 => "H.264",
        FRAME_FMT_H265 => "H.265",
        FRAME_FMT_AV1 => "AV1",
        FRAME_FMT_VP8 => "VP8",
        FRAME_FMT_VP9 => "VP9",
        _ => "?",
    }
}

/// ── Stream control codes ──────────────────────────────────────────────

/// Sent as a zero-length frame; forces the decoder to reset.
pub const STREAM_CTRL_RESYNC: u32 = 0;

/// Sent as a zero-length frame before a keyframe.
pub const STREAM_CTRL_KEYFRAME: u32 = 1;

/// Magic sentinel: length == u32::MAX means no more frames (stream end).
pub const STREAM_END_MARKER: u32 = u32::MAX;

/// ── Adaptive quality thresholds ────────────────────────────────────────

/// Minimum interval between two successive adaptive quality updates
/// to avoid "flapping" (ms).
pub const ADAPTIVE_COOLDOWN_MS: u64 = 3_000;

/// Client-side stats: how many seconds to average throughput over.
pub const THROUGHPUT_WINDOW_SECS: u64 = 3;

/// ── Reconnection policy ────────────────────────────────────────────────

/// Base backoff delay for reconnect attempts (ms).
pub const RECONNECT_BASE_DELAY_MS: u64 = 500;

/// Maximum backoff delay (ms).
pub const RECONNECT_MAX_DELAY_MS: u64 = 15_000;

/// Multiplier for exponential backoff per attempt.
pub const RECONNECT_BACKOFF_FACTOR: u32 = 2;

/// Maximum number of reconnection attempts before giving up.
pub const RECONNECT_MAX_ATTEMPTS: u32 = 10;

/// ── Frame pacing constants ─────────────────────────────────────────────

/// Maximum consecutive frame drops before forcing a quality downgrade.
pub const MAX_CONSECUTIVE_DROPS: u32 = 8;

/// Minimum interval (ms) between quality downgrade steps.
pub const QUALITY_DOWNGRADE_INTERVAL_MS: u64 = 500;

/// ── Encoder helpers ────────────────────────────────────────────────────

/// Build a compact stream frame with format byte.
/// `data` is the payload (RGBA pixels or encoded codec data).
pub fn build_stream_frame(
    orig_w: u32,
    orig_h: u32,
    frame_w: u32,
    frame_h: u32,
    data: &[u8],
    fmt: u8,
) -> Vec<u8> {
    let payload_len = STREAM_FRAME_HDR_SIZE + data.len();
    let mut buf = Vec::with_capacity(STREAM_FRAME_LEN_SIZE + payload_len);
    buf.extend_from_slice(&(payload_len as u32).to_be_bytes());
    buf.push(fmt);
    buf.extend_from_slice(&(orig_w as u16).to_be_bytes());
    buf.extend_from_slice(&(orig_h as u16).to_be_bytes());
    buf.extend_from_slice(&(frame_w as u16).to_be_bytes());
    buf.extend_from_slice(&(frame_h as u16).to_be_bytes());
    buf.extend_from_slice(data);
    buf
}

/// Build a stream control frame (resync or keyframe marker).
pub fn build_stream_ctrl_frame(code: u32) -> Vec<u8> {
    code.to_be_bytes().to_vec()
}

/// Build a stream-end frame.
pub fn build_stream_end_frame() -> Vec<u8> {
    STREAM_END_MARKER.to_be_bytes().to_vec()
}

/// Parse a stream frame header from the prefix bytes.
/// Returns `(format, orig_w, orig_h, frame_w, frame_h, data_offset)`.
pub fn parse_stream_frame_header(prefix: &[u8]) -> Option<(u8, u32, u32, u32, u32, usize)> {
    if prefix.len() < STREAM_FRAME_PREFIX {
        return None;
    }
    let fmt = prefix[4];
    let orig_w = u16::from_be_bytes([prefix[5], prefix[6]]) as u32;
    let orig_h = u16::from_be_bytes([prefix[7], prefix[8]]) as u32;
    let frame_w = u16::from_be_bytes([prefix[9], prefix[10]]) as u32;
    let frame_h = u16::from_be_bytes([prefix[11], prefix[12]]) as u32;
    Some((fmt, orig_w, orig_h, frame_w, frame_h, STREAM_FRAME_PREFIX))
}

/// ── Adaptive feedback via signaling ────────────────────────────────────

/// Adaptive quality feedback sent from viewer to host via signaling.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AdaptiveFeedback {
    /// Current measured FPS on the client side.
    pub measured_fps: f64,
    /// Estimated throughput in bytes/sec over the last window.
    pub throughput_bps: f64,
    /// SCTP buffered amount reported by the host (if known).
    pub buffered_amount: u64,
    /// Number of frames dropped by the client due to decoder overflow / timeout.
    pub dropped_frames: u32,
    /// Client-recommended quality level (0-100).
    pub recommended_quality: Option<u32>,
    /// Client-recommended max dimension.
    pub recommended_max_dim: Option<u32>,
    /// Client-recommended FPS.
    pub recommended_fps: Option<u32>,
}

impl Default for AdaptiveFeedback {
    fn default() -> Self {
        Self {
            measured_fps: 0.0,
            throughput_bps: 0.0,
            buffered_amount: 0,
            dropped_frames: 0,
            recommended_quality: None,
            recommended_max_dim: None,
            recommended_fps: None,
        }
    }
}
