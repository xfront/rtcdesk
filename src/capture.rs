//! Screen capture module for desktop sharing.
//!
//! Captures the host's screen at a configurable frame rate and
//! encodes frames as JPEG for transmission over the data channel.
//! JPEG lossy compression keeps frames around 30-60 KB for 720p,
//! which fits in 2-4 SCTP chunks (vs. ~500 KB for PNG = 34 chunks).

use anyhow::{Context, Result};
use image::{DynamicImage, ImageEncoder};
use log::{debug, info};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::Duration;
use xcap::Monitor;

/// A single captured frame with metadata.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Encoded JPEG data.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
}

/// Screen capturer that periodically grabs desktop frames.
/// All mutable config fields use atomics so they can be updated behind `Arc`.
pub struct ScreenCapturer {
    /// The monitor to capture.
    monitor: Monitor,
    /// JPEG encoding quality (0-100), default 30.
    jpeg_quality: AtomicU8,
    /// Scale down frames whose width or height exceeds this value.
    max_dimension: AtomicU32,
}

// xcap::Monitor does not implement Send + Sync on Windows because it holds
// HMONITOR / DC handles.  All Monitor access happens inside `capture_frame()`
// which is always called via `tokio::spawn_blocking` — each call internally
// creates fresh GDI resources and the raw handles themselves are safe to
// share across threads as long as they are not accessed concurrently.
#[cfg(not(target_os = "linux"))]
unsafe impl Send for ScreenCapturer {}
#[cfg(not(target_os = "linux"))]
unsafe impl Sync for ScreenCapturer {}

impl ScreenCapturer {
    /// Create a new screen capturer for the primary monitor.
    pub fn new() -> Result<Self> {
        let monitors = Monitor::all().context("failed to enumerate monitors")?;

        let monitor = monitors
            .into_iter()
            .next()
            .context("no monitors detected")?;

        Ok(Self {
            monitor,
            jpeg_quality: AtomicU8::new(30),
            max_dimension: AtomicU32::new(1920),
        })
    }

    /// Set JPEG compression quality (0-100).
    /// Lower = smaller file, worse quality. Recommended range: 60-85.
    pub fn set_quality(&self, quality: u8) {
        self.jpeg_quality
            .store(quality.clamp(0, 100), Ordering::Relaxed);
    }

    /// Set maximum dimension for scaling.
    pub fn set_max_dimension(&self, dim: u32) {
        self.max_dimension.store(dim, Ordering::Relaxed);
    }

    /// Read current JPEG quality.
    pub fn quality(&self) -> u8 {
        self.jpeg_quality.load(Ordering::Relaxed)
    }

    /// Read current max dimension.
    pub fn max_dimension(&self) -> u32 {
        self.max_dimension.load(Ordering::Relaxed)
    }

    /// Capture a single frame and encode as JPEG.
    pub fn capture_frame(&self) -> Result<CapturedFrame> {
        let t0 = std::time::Instant::now();

        // xcap returns image::RgbaImage on Linux (X11)
        let rgba = self
            .monitor
            .capture_image()
            .context("failed to capture screen")?;
        let t1 = t0.elapsed();

        let (width, height) = (rgba.width(), rgba.height());
        let max_dim = self.max_dimension.load(Ordering::Relaxed);
        let mut dyn_image = DynamicImage::ImageRgba8(rgba);

        // Scale down if too large
        if width > max_dim || height > max_dim {
            dyn_image = dyn_image.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3);
            debug!(
                "Scaled frame from {}x{} to {}x{}",
                width,
                height,
                dyn_image.width(),
                dyn_image.height(),
            );
        }

        // Convert RGBA → RGB (JPEG doesn't support alpha channel)
        let rgb = dyn_image.to_rgb8();

        let jpeg_q = self.jpeg_quality.load(Ordering::Relaxed);
        // JPEG encode with configured quality
        let mut buffer = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, jpeg_q);
        encoder.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;

        let jpeg_data = buffer.into_inner();
        let t3 = t0.elapsed();
        let kb = jpeg_data.len() / 1024;
        info!(
            "Capture: {}x{} → {} KB, X11={:.0}ms convert+encode={:.0}ms total={:.0}ms (JPEG Q={})",
            rgb.width(),
            rgb.height(),
            kb,
            t1.as_millis(),
            (t3 - t1).as_millis(),
            t3.as_millis(),
            jpeg_q,
        );

        Ok(CapturedFrame {
            data: jpeg_data,
            width: rgb.width(),
            height: rgb.height(),
        })
    }

    /// Start continuous capture, yielding frames at the given interval.
    pub async fn capture_loop<F>(&self, interval: Duration, mut on_frame: F) -> Result<()>
    where
        F: FnMut(CapturedFrame) + Send,
    {
        let mut interval_timer = tokio::time::interval(interval);

        loop {
            interval_timer.tick().await;
            match self.capture_frame() {
                Ok(frame) => on_frame(frame),
                Err(e) => log::warn!("Capture error: {}", e),
            }
        }
    }

    /// Returns the monitor resolution.
    pub fn resolution(&self) -> (u32, u32) {
        (
            self.monitor.width().unwrap(),
            self.monitor.height().unwrap(),
        )
    }
}
