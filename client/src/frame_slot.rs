//! Shared frame slot for zero-copy video display via custom wry protocol.
//!
//! The decode thread writes decoded BGRA pixels into a `FrameSlot`
//! (behind an `Arc<Mutex>`).  A custom wry URI scheme (`rtcdesk://frame`)
//! serves the latest frame as a BMP image, which the WebView's `<img>`
//! tag renders directly — no JPEG re-encoding, no base64 overhead.

use std::sync::Mutex;

/// Shared frame storage accessible by both the decode thread and
/// the custom wry protocol handler.
pub type SharedFrameSlot = Mutex<FrameSlot>;

/// Latest decoded frame, stored as a BMP-ready buffer.
///
/// The `bmp_data` field contains a valid BMP file (54-byte header +
/// BGRA pixel data) that can be served directly as an HTTP response.
pub struct FrameSlot {
    /// Complete BMP file data (header + pixels), ready to serve.
    pub bmp_data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Monotonically increasing sequence counter.
    pub sequence: u64,
}

impl FrameSlot {
    pub fn new() -> Self {
        Self {
            bmp_data: Vec::new(),
            width: 0,
            height: 0,
            sequence: 0,
        }
    }

    /// Update the frame slot with new BGRA pixel data.
    ///
    /// Constructs a BMP file in-place: 14-byte file header +
    /// 40-byte BITMAPINFOHEADER (with negative biHeight for
    /// top-to-bottom row order) + raw BGRA pixels.
    pub fn update(&mut self, bgra: &[u8], w: u32, h: u32) {
        self.width = w;
        self.height = h;
        self.sequence = self.sequence.wrapping_add(1);

        let row_stride = (w as usize) * 4;
        let pixel_size = row_stride * (h as usize);
        let bmp_size = 54 + pixel_size;

        // Resize buffer if needed
        if self.bmp_data.len() != bmp_size {
            self.bmp_data.resize(bmp_size, 0);
        }

        // Write BMP file header (14 bytes)
        let hdr = &mut self.bmp_data[..14];
        hdr[0..2].copy_from_slice(b"BM");                        // Signature
        let file_size = bmp_size as u32;
        hdr[2..6].copy_from_slice(&file_size.to_le_bytes());     // File size
        // hdr[6..10] = reserved (0)
        hdr[10..14].copy_from_slice(&54u32.to_le_bytes());       // Pixel data offset

        // Write BITMAPINFOHEADER (40 bytes)
        let dib = &mut self.bmp_data[14..54];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());        // Header size
        dib[4..8].copy_from_slice(&(w as i32).to_le_bytes());   // Width
        // Negative height = top-to-bottom row order (no row flip needed)
        let neg_height: i32 = -(h as i32);
        dib[8..12].copy_from_slice(&neg_height.to_le_bytes()); // Height (negative)
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());       // Planes
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());      // Bits per pixel (BGRA = 32)
        // dib[16..20] = compression (0 = BI_RGB)
        let image_size = pixel_size as u32;
        dib[20..24].copy_from_slice(&image_size.to_le_bytes()); // Image size
        // dib[24..28] = X pixels per meter (0)
        // dib[28..32] = Y pixels per meter (0)
        // dib[32..36] = colors used (0)
        // dib[36..40] = important colors (0)

        // Copy BGRA pixel data directly (BMP native byte order)
        let pixels = &mut self.bmp_data[54..];
        let copy_len = std::cmp::min(pixels.len(), bgra.len());
        pixels[..copy_len].copy_from_slice(&bgra[..copy_len]);
    }
}

/// Build the custom protocol handler name.
pub const FRAME_PROTOCOL_NAME: &str = "rtcdesk";

/// Build the frame URL for the `<img>` tag.
/// The `seq` query parameter forces the browser to re-fetch
/// (bypasses cache) on each frame update.
pub fn frame_url(sequence: u64) -> String {
    format!("rtcdesk://frame?seq={}", sequence)
}
