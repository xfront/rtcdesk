//! Screen capture module for desktop sharing.
//!
//! Uses xcap's `VideoRecorder` API to capture the host screen as a
//! continuous RGBA frame stream.  Frames are received via an MPSC channel.
//!
//! Encoding is handled by the video track streaming loop in `peer.rs`,
//! which converts RGBA → H.264 and sends RTP packets via WebRTC Video Track.

use anyhow::{Context, Result};
use log::info;
use std::sync::Mutex;
use xcap::{Frame, Monitor, VideoRecorder};

/// A single captured frame with metadata.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Raw RGBA pixel data (width × height × 4 bytes).
    pub data: Vec<u8>,
    /// Monitor width in pixels.
    pub width: u32,
    /// Monitor height in pixels.
    pub height: u32,
}

/// Screen capturer using xcap's VideoRecorder for continuous frame streaming.
///
/// The video recorder runs a background thread that pushes RGBA frames into
/// an MPSC channel.  `capture_frame()` drains the channel to keep latency
/// minimal and returns the latest frame.
pub struct ScreenCapturer {
    /// The monitor being captured (retained for resolution queries).
    monitor: Monitor,
    /// Video recorder controller (start/stop).
    video_recorder: VideoRecorder,
    /// Receiver end of the frame channel — drained per `capture_frame()` call.
    frame_rx: Mutex<std::sync::mpsc::Receiver<Frame>>,
}

// xcap::Monitor does not implement Send + Sync on Windows because it holds
// HMONITOR / DC handles.  All Monitor access happens inside the
// VideoRecorder's internal thread, and resolution() only reads static
// properties — so the unsafe impl is safe as long as Monitor is never
// accessed concurrently.
#[cfg(not(target_os = "linux"))]
unsafe impl Send for ScreenCapturer {}
#[cfg(not(target_os = "linux"))]
unsafe impl Sync for ScreenCapturer {}

impl ScreenCapturer {
    /// Create a new screen capturer for the primary monitor.
    ///
    /// Initialises the `VideoRecorder` and starts capturing immediately.
    /// Frames are pushed into an internal MPSC channel and consumed
    /// on demand by `capture_frame()`.
    pub fn new() -> Result<Self> {
        let monitors = Monitor::all().context("failed to enumerate monitors")?;
        let monitor = monitors
            .into_iter()
            .next()
            .context("no monitors detected")?;

        let (video_recorder, frame_rx) = monitor
            .video_recorder()
            .context("failed to create video recorder")?;

        video_recorder
            .start()
            .context("failed to start video recorder")?;

        info!(
            "Video recorder started for {}x{}",
            monitor.width().unwrap_or(0),
            monitor.height().unwrap_or(0),
        );

        Ok(Self {
            monitor,
            video_recorder,
            frame_rx: Mutex::new(frame_rx),
        })
    }

    /// Capture the latest frame from the video recorder.
    ///
    /// Blocks until a frame arrives, then drains any backlog to return
    /// the most recent frame (minimum latency).
    /// The blocking nature is safe because this method is always called
    /// inside `tokio::task::spawn_blocking`.
    pub fn capture_frame(&self) -> Result<CapturedFrame> {
        let rx = self.frame_rx.lock().unwrap();
        let mut latest = rx
            .recv()
            .map_err(|e| anyhow::anyhow!("video frame recv error: {e}"))?;
        while let Ok(f) = rx.try_recv() {
            latest = f;
        }

        Ok(CapturedFrame {
            data: latest.raw,
            width: latest.width,
            height: latest.height,
        })
    }

    /// Returns the monitor resolution.
    pub fn resolution(&self) -> (u32, u32) {
        let w = self.monitor.width().unwrap_or(1920);
        let h = self.monitor.height().unwrap_or(1080);
        (w, h)
    }
}

impl Drop for ScreenCapturer {
    fn drop(&mut self) {
        if let Err(e) = self.video_recorder.stop() {
            log::warn!("Error stopping video recorder: {:?}", e);
        }
        info!("Video recorder stopped");
    }
}
