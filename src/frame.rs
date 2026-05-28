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
pub const CHUNK_WIRE_SIZE: usize = 12 + CHUNK_DATA_SIZE;

// ── Sender helpers ────────────────────────────────────────────────────────

/// Split a frame payload into ordered chunks ready for `dc.send()`.
pub fn chunk_frame(frame_id: u32, data: &[u8]) -> Vec<Vec<u8>> {
    let total = data.len().div_ceil(CHUNK_DATA_SIZE) as u32;
    let mut chunks = Vec::with_capacity(total as usize);

    for (i, piece) in data.chunks(CHUNK_DATA_SIZE).enumerate() {
        let mut chunk = Vec::with_capacity(12 + piece.len());
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
        if raw.len() < 12 {
            return None; // too short to be a valid chunk
        }

        let frame_id = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let chunk_ix = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let total = u32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]);
        let data = &raw[12..];

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
