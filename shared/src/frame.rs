//! Frame chunking / reassembly for large frames over SCTP data channels.
//!
//! SCTP data channels have a maximum message size (~16 KiB by default).
//! Desktop-screenshot PNG frames easily exceed this, so we split each frame
//! into chunks at the sender and reassemble them at the receiver.
//!
//! **Chunk wire format** (12-byte header, all big-endian):
//! ```text
//! ┌──────────┬──────────┬─────────────┬──────────────┐
//! │ frame_id │ chunk_ix │ total_chunks│ chunk_data … │
//! │  u32 BE  │  u32 BE  │   u32 BE    │              │
//! └──────────┴──────────┴─────────────┴──────────────┘
//!   0…3        4…7        8…11          12…
//! ```

use std::collections::HashMap;

/// Maximum payload size per SCTP message (well below the ~16 KiB default).
pub const CHUNK_DATA_SIZE: usize = 15 * 1024; // 15 KiB

/// Total wire bytes per chunk: 12-byte header + data.
pub const CHUNK_WIRE_SIZE: usize = CHUNK_HDR_SIZE + CHUNK_DATA_SIZE;

// ── Wire format sizes ──────────────────────────────────────────────────────

/// Frame header size:
///   orig_w (u32 BE) + orig_h (u32 BE) + frame_w (u32 BE) + frame_h (u32 BE).
///
/// `orig_*` are the actual remote monitor dimensions (for mouse coordinate mapping);
/// `frame_*` are the possibly down-scaled JPEG dimensions (for display layout).
pub const FRAME_HDR_SIZE: usize = 16;

/// Byte offset of the original (unscaled) width in the frame header.
pub const FRAME_ORIG_W_OFF: usize = 0;
/// Byte offset of the original (unscaled) height.
pub const FRAME_ORIG_H_OFF: usize = 4;
/// Byte offset of the scaled frame width.
pub const FRAME_W_OFF: usize = 8;
/// Byte offset of the scaled frame height.
pub const FRAME_H_OFF: usize = 12;
/// Chunk header size: frame_id (u32 BE) + chunk_ix (u32 BE) + total_chunks (u32 BE).
pub const CHUNK_HDR_SIZE: usize = 12;
/// File chunk size for clipboard transfer.
pub const FILE_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB

// ── Input event protocol tags ──────────────────────────────────────────────

/// Mouse event tag byte.
pub const TAG_MOUSE: u8 = 0x01;
/// Key event tag byte.
pub const TAG_KEY: u8 = 0x02;
/// Wheel event tag byte.
pub const TAG_WHEEL: u8 = 0x03;

/// Mouse event wire size: tag(1) + x(8) + y(8) + btn(1) + pressed(1).
pub const MOUSE_EVENT_SIZE: usize = 19;
/// Byte offset of the x coordinate within a mouse event payload.
pub const MOUSE_X_OFF: usize = 1;
/// Byte offset of the y coordinate (x occupies 8 bytes).
pub const MOUSE_Y_OFF: usize = 9;
/// Byte offset of the button field.
pub const MOUSE_BTN_OFF: usize = 17;
/// Byte offset of the pressed boolean.
pub const MOUSE_PRESSED_OFF: usize = 18;

/// Key event wire size: tag(1) + key_code(4) + pressed(1).
pub const KEY_EVENT_SIZE: usize = 6;
/// Byte offset of the key_code (u32 LE) field.
pub const KEY_CODE_OFF: usize = 1;
/// Byte offset of the pressed boolean.
pub const KEY_PRESSED_OFF: usize = 5;

/// Wheel event wire size: tag(1) + dx(i32 LE, 4) + dy(i32 LE, 4).
pub const WHEEL_EVENT_SIZE: usize = 9;
/// Byte offset of the dx (i32 LE) field.
pub const WHEEL_DX_OFF: usize = 1;
/// Byte offset of the dy (i32 LE) field (dx occupies 4 bytes).
pub const WHEEL_DY_OFF: usize = 5;

// ── Mouse button codes (sent over the wire) ────────────────────────────────

pub const BTN_LEFT: u8 = 0;
pub const BTN_MIDDLE: u8 = 1;
pub const BTN_RIGHT: u8 = 2;
/// Sentinel value: mouse-move only, no button action on the host.
pub const BTN_MOVE: u8 = 0xFF;

// ── Clipboard protocol tags ────────────────────────────────────────────────

/// Text clipboard data from viewer.
pub const CLIP_TAG_TEXT: u8 = 0x01;
/// File list header in clipboard transfer.
pub const CLIP_TAG_FILE_LIST: u8 = 0x03;
/// File data chunk.
pub const CLIP_TAG_FILE_CHUNK: u8 = 0x04;
/// Clipboard transfer complete marker.
pub const CLIP_TAG_DONE: u8 = 0x05;
/// Image clipboard data (PNG-encoded).
/// Wire format: [CLIP_TAG_IMAGE:1][width:u32le][height:u32le][png_data...]
pub const CLIP_TAG_IMAGE: u8 = 0x06;
/// Image clipboard header size after tag: width(4) + height(4).
pub const CLIP_IMAGE_HDR_SIZE: usize = 8;
/// Max image size to sync via clipboard (8 MB of PNG, larger images skipped).
pub const CLIP_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Clipboard file list header size after tag: num_files(2) + per_entry(name_len:2 + name:N + size:8).
pub const CLIP_FILE_LIST_MIN_LEN: usize = 2;
/// Clipboard chunk header: file_idx(2) + offset(8) + len(4).
pub const CLIP_CHUNK_HDR_SIZE: usize = 14;
/// Clipboard chunk wire message size: tag(1) + CLIP_CHUNK_HDR_SIZE.
pub const CLIP_CHUNK_MSG_SIZE: usize = 15;
/// Initial buffer capacity for file list header.
pub const CLIP_FILE_LIST_BUF: usize = 256;

// ── Timeout / poll intervals (shared across host & viewer) ─────────────────

pub const INPUT_POLL_MS: u64 = 50;
pub const CLIPBOARD_POLL_MS: u64 = 50;
pub const CLIPBOARD_WRITE_POLL_MS: u64 = 200;
pub const DC_OPEN_POLL_MS: u64 = 100;
pub const DC_OPEN_TIMEOUT_SECS: u64 = 10;
pub const ICE_TIMEOUT_SECS: u64 = 30;
pub const INTER_CHUNK_DELAY_MS: u64 = 2;
pub const VIEWER_FRAME_POLL_MS: u64 = 5;

// ── Key code constants (shared VK / scan-code values) ──────────────────────

pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_ALT: u32 = 0x12;
pub const VK_PAUSE: u32 = 0x13;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_PRIOR: u32 = 0x21; // PageUp
pub const VK_NEXT: u32 = 0x22; // PageDown
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_PRINT: u32 = 0x2C; // PrintScreen
pub const VK_INSERT: u32 = 0x2D;
pub const VK_DELETE: u32 = 0x7F;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_NUMLOCK: u32 = 0x90;
pub const VK_SCROLL: u32 = 0x91;
pub const VK_META: u32 = 0x100;
pub const VK_V: u32 = 0x76;
pub const VK_F1: u32 = 0x70;
pub const VK_F2: u32 = 0x71;
pub const VK_F3: u32 = 0x72;
pub const VK_F4: u32 = 0x73;
pub const VK_F5: u32 = 0x74;
pub const VK_F6: u32 = 0x75;
pub const VK_F7: u32 = 0x76; // conflicts with VK_V — see key_event_to_code
pub const VK_F8: u32 = 0x77;
pub const VK_F9: u32 = 0x78;
pub const VK_F10: u32 = 0x79;
pub const VK_F11: u32 = 0x7A;
pub const VK_F12: u32 = 0x7B;

// ── GUI defaults ───────────────────────────────────────────────────────────

pub const DEFAULT_WINDOW_W: u32 = 1280;
pub const DEFAULT_WINDOW_H: u32 = 800;
pub const MIN_WINDOW_W: u32 = 800;
pub const MIN_WINDOW_H: u32 = 600;
pub const TOOLBAR_THRESHOLD: f64 = 80.0;
pub const DEFAULT_QUALITY_IDX: usize = 1;
pub const DEFAULT_FRAME_W: u32 = 1280;
pub const DEFAULT_FRAME_H: u32 = 720;
pub const MIN_SCALE_FACTOR: f64 = 1.0;
/// Custom titlebar height in CSS pixels. Must match `.titlebar { height: 40px; }` in app.css.
pub const TITLEBAR_HEIGHT: f64 = 40.0;
pub const DEFAULT_STUN: &str = "stun:stun.l.google.com:19302";
pub const DEFAULT_FPS: u32 = 15;
pub const DEFAULT_CAPTURE_DIM: u32 = 1280;
pub const SIGNAL_STARTUP_DELAY_MS: u64 = 300;
pub const DEFAULT_JPEG_QUALITY: u8 = 30;
pub const DEFAULT_MAX_DIMENSION: u32 = 1920;
pub const SIGNAL_BROADCAST_CAP: usize = 64;
pub const STUN_QUERY_TIMEOUT_SECS: u64 = 40;

// ── Chat / stats / file transfer constants ──────────────────────────────
// 注意：聊天功能使用独立 DataChannel + JSON 序列化，不使用 tagged binary protocol
/// Stats refresh interval (ms)
pub const STATS_REFRESH_MS: u64 = 1000;
/// Screenshot save directory name
pub const SCREENSHOT_DIR: &str = "rtcdesk";
/// File size limit for single file transfer (500 MB)
pub const FILE_TRANSFER_MAX_SIZE: u64 = 500 * 1024 * 1024;

// ── Sender helpers ────────────────────────────────────────────────────────

/// Split a frame payload into ordered chunks ready for `dc.send()`.
pub fn chunk_frame(frame_id: u32, data: &[u8]) -> Vec<Vec<u8>> {
    let total = data.len().div_ceil(CHUNK_DATA_SIZE) as u32;
    let mut chunks = Vec::with_capacity(total as usize);

    for (i, piece) in data.chunks(CHUNK_DATA_SIZE).enumerate() {
        let mut chunk = Vec::with_capacity(CHUNK_HDR_SIZE + piece.len());
        chunk.extend_from_slice(&frame_id.to_be_bytes());
        chunk.extend_from_slice(&(i as u32).to_be_bytes());
        chunk.extend_from_slice(&total.to_be_bytes());
        chunk.extend_from_slice(piece);
        chunks.push(chunk);
    }

    chunks
}

// ── Receiver helper ───────────────────────────────────────────────────────

/// Reassembles multi-chunk frames received over a data channel.
pub struct FrameAssembler {
    /// `frame_id → (total_chunks, map<chunk_index, chunk_data>)`
    pending: HashMap<u32, (u32, HashMap<u32, Vec<u8>>)>,
    /// The smallest frame_id we still care about.
    min_frame_id: u32,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            min_frame_id: 0,
        }
    }

    /// Feed in a raw chunk (the full SCTP message).
    ///
    /// Returns `Some(data)` when a frame has been fully reassembled,
    /// or `None` if more chunks are needed.
    pub fn push(&mut self, raw: &[u8]) -> Option<Vec<u8>> {
        if raw.len() < CHUNK_HDR_SIZE {
            return None; // too short to be a valid chunk
        }

        let frame_id = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let chunk_ix = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let total = u32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]);
        let data = &raw[CHUNK_HDR_SIZE..];

        // Drop stale frames (id older than min_frame_id)
        if frame_id < self.min_frame_id {
            return None;
        }

        if total == 0 || chunk_ix >= total {
            return None;
        }

        let entry = self
            .pending
            .entry(frame_id)
            .or_insert_with(|| (total, HashMap::new()));

        entry.1.insert(chunk_ix, data.to_vec());

        // All chunks received?
        if entry.1.len() as u32 == entry.0 {
            let (_, chunks) = self.pending.remove(&frame_id).unwrap();

            // Merge in order
            let mut merged = Vec::new();
            for i in 0..total {
                if let Some(piece) = chunks.get(&i) {
                    merged.extend_from_slice(piece);
                } else {
                    // Should never happen, but guard against it.
                    return None;
                }
            }

            // Clean up stale entries: drop any frame_ids ≤ this one.
            self.min_frame_id = frame_id.wrapping_add(1);
            self.pending.retain(|&id, _| id >= self.min_frame_id);

            return Some(merged);
        }

        None
    }
}
