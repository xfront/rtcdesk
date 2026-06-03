//! H264 video pipeline: RTP depacketize → decode → BGRA output.
//!
//! Following RustDesk's approach: simple hardware decode with CPU readback,
//! delivering raw BGRA pixels to the display via a shared FrameSlot.
//!
//! Architecture:
//! - RtpDepacketizer: fast, runs in the on_track async loop
//!   (just accumulates RTP fragments, no decode)
//! - FrameDecoder: slow, runs in a dedicated OS thread
//!   (VA-API hardware / OpenH264 software fallback)
//!
//! The decode thread writes BGRA pixels to a FrameSlot (shared via Arc<Mutex>).
//! A custom wry protocol serves the FrameSlot content as BMP to the WebView.

use bytes::Bytes;
use log::{info, warn};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::packetizer::Depacketizer;
#[cfg(feature = "vaapi")]
use ffmpeg_sys_next as ff;

// ─── Decoded frame ──────────────────────────────────────────────────

/// A decoded video frame in BGRA pixel format, ready for display.
///
/// BGRA is the native byte order for BMP files and many graphics APIs.
/// Each pixel is 4 bytes: Blue, Green, Red, Alpha (0xFF).
pub struct DecodedFrame {
    pub bgra_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ─── RTP Depacketizer (fast, runs in on_track loop) ────────────────

/// RTP depacketizer for H264 video.
///
/// Accumulates depacketized NAL data across RTP packets.
/// On the marker bit (frame boundary), returns the complete
/// H264 Annex-B data for the frame.
///
/// This is intentionally FAST — no GPU or CPU decoding.
/// It should be called for every RTP packet in the on_track loop.
pub struct RtpDepacketizer {
    depacketizer: H264Packet,
    /// Accumulated depacketized H264 Annex-B data for the current frame.
    frame_buffer: Vec<u8>,
}

impl RtpDepacketizer {
    pub fn new() -> Self {
        Self {
            depacketizer: H264Packet::default(),
            frame_buffer: Vec::new(),
        }
    }

    /// Process an RTP packet.
    ///
    /// Returns `(Some(h264_data), false)` on the marker bit (end of frame)
    /// with the complete H264 Annex-B data for that frame.
    /// Returns `(None, false)` for intermediate packets (FU-A continuation, etc.).
    /// Returns `(None, true)` if a depacketization error occurred and a
    /// keyframe is needed.
    pub fn process(&mut self, payload: &[u8], marker: bool) -> (Option<Vec<u8>>, bool) {
        let payload_bytes = Bytes::copy_from_slice(payload);
        match self.depacketizer.depacketize(&payload_bytes) {
            Ok(data) => {
                if data.is_empty() {
                    return (None, false);
                }
                self.frame_buffer.extend_from_slice(&data);
            }
            Err(e) => {
                warn!("[Depacketizer] H264 depacketize error: {}", e);
                self.depacketizer = H264Packet::default();
                self.frame_buffer.clear();
                return (None, true); // need keyframe
            }
        }

        // Only return data on marker bit (frame boundary)
        if !marker {
            return (None, false);
        }

        let frame_data = std::mem::take(&mut self.frame_buffer);
        (Some(frame_data), false)
    }
}

// ─── Frame Decoder (slow, runs in decode thread) ────────────────────

/// Frame decoder: tries VA-API hardware decode first,
/// falls back to OpenH264 software decoding.
///
/// Following RustDesk's approach: hardware decode → CPU readback → BGRA.
/// No DMA-BUF, no EGL, no GPU rendering — just simple CPU readback.
pub enum FrameDecoder {
    #[cfg(feature = "vaapi")]
    Vaapi(VaapiFrameDecoder),
    Software(SoftwareFrameDecoder),
}

impl FrameDecoder {
    /// Try hardware decoders first; fall back to software.
    pub fn new() -> Self {
        // Try VA-API hardware decode (Intel, AMD on Linux)
        #[cfg(feature = "vaapi")]
        {
            match VaapiFrameDecoder::new() {
                Ok(vaapi) => {
                    info!("[Decoder] VA-API hardware decoder initialized");
                    return FrameDecoder::Vaapi(vaapi);
                }
                Err(e) => {
                    warn!("[Decoder] VA-API not available: {}", e);
                }
            }
        }

        // Fall back to OpenH264 software decoder
        warn!("[Decoder] Falling back to OpenH264 software decoder");
        FrameDecoder::Software(
            SoftwareFrameDecoder::new().expect("OpenH264 init failed"),
        )
    }

    /// Decode a complete H264 Annex-B frame.
    pub fn decode_frame(&mut self, h264_data: &[u8]) -> (Option<DecodedFrame>, bool) {
        match self {
            #[cfg(feature = "vaapi")]
            FrameDecoder::Vaapi(vaapi) => vaapi.decode(h264_data),
            FrameDecoder::Software(sw) => sw.decode(h264_data),
        }
    }

    /// Signal that frames were lost (dropped) so the decoder can
    /// reset its state.
    pub fn signal_data_loss(&mut self) {
        match self {
            #[cfg(feature = "vaapi")]
            FrameDecoder::Vaapi(vaapi) => vaapi.flush(),
            FrameDecoder::Software(sw) => sw.reset(),
        }
    }
}

// ─── Software Frame Decoder (OpenH264 fallback) ──────────────────────

/// OpenH264 software decoder fallback.
///
/// Decodes H264 to YUV, then converts to BGRA for display.
pub struct SoftwareFrameDecoder {
    decoder: Decoder,
    has_decoded_frame: bool,
}

impl SoftwareFrameDecoder {
    pub fn new() -> Result<Self, openh264::Error> {
        Ok(Self {
            decoder: Decoder::new()?,
            has_decoded_frame: false,
        })
    }

    /// Decode a complete H264 Annex-B frame to BGRA.
    pub fn decode(&mut self, h264_data: &[u8]) -> (Option<DecodedFrame>, bool) {
        let mut need_keyframe = false;

        // Split Annex-B bytestream into individual NAL units
        let nals: Vec<Vec<u8>> = openh264::nal_units(h264_data)
            .map(|n| n.to_vec())
            .collect();

        let mut last_bgra: Option<(Vec<u8>, u32, u32)> = None;

        for nal in nals {
            match self.decoder.decode(&nal) {
                Ok(Some(yuv)) => {
                    let (w, h) = yuv.dimensions();
                    if w > 0 && h > 0 {
                        // Convert YUV → RGB via openh264, then RGB → BGRA
                        let rgb_len = (w as usize) * (h as usize) * 3;
                        let mut rgb = vec![0u8; rgb_len];
                        yuv.write_rgb8(&mut rgb);
                        // Convert RGB → BGRA (swap R↔B, add alpha=0xFF)
                        let bgra = rgb_to_bgra(&rgb);
                        last_bgra = Some((bgra, w as u32, h as u32));
                    }
                }
                Ok(None) => {
                    // Normal: more NALs needed to complete a frame
                }
                Err(e) => {
                    if !self.has_decoded_frame {
                        need_keyframe = true;
                    } else {
                        warn!(
                            "[SW Decoder] decode error: {} — NAL ({} bytes)",
                            e,
                            nal.len()
                        );
                    }
                }
            }
        }

        // Flush any remaining frames the decoder is holding
        match self.decoder.flush_remaining() {
            Ok(frames) => {
                for yuv in frames {
                    let (w, h) = yuv.dimensions();
                    if w > 0 && h > 0 {
                        let rgb_len = (w as usize) * (h as usize) * 3;
                        let mut rgb = vec![0u8; rgb_len];
                        yuv.write_rgb8(&mut rgb);
                        let bgra = rgb_to_bgra(&rgb);
                        last_bgra = Some((bgra, w as u32, h as u32));
                    }
                }
            }
            Err(e) => {
                if !self.has_decoded_frame {
                    need_keyframe = true;
                } else {
                    warn!("[SW Decoder] flush error: {}", e);
                }
            }
        }

        if let Some((bgra, w, h)) = last_bgra {
            self.has_decoded_frame = true;
            return (Some(DecodedFrame {
                bgra_data: bgra,
                width: w,
                height: h,
            }), false);
        }

        (None, need_keyframe)
    }

    /// Reset the decoder — creates a fresh OpenH264 Decoder.
    pub fn reset(&mut self) {
        if let Ok(decoder) = Decoder::new() {
            self.decoder = decoder;
            self.has_decoded_frame = false;
            info!("[SW Decoder] Reset — waiting for keyframe");
        }
    }
}

/// Convert RGB8 data to BGRA (swap R↔B, add alpha=0xFF).
///
/// Input:  RGB packed (3 bytes per pixel)
/// Output: BGRA packed (4 bytes per pixel)
fn rgb_to_bgra(rgb: &[u8]) -> Vec<u8> {
    let pixel_count = rgb.len() / 3;
    let mut bgra = Vec::with_capacity(pixel_count * 4);
    for pixel in rgb.chunks_exact(3) {
        bgra.push(pixel[2]); // B
        bgra.push(pixel[1]); // G
        bgra.push(pixel[0]); // R
        bgra.push(0xFF);     // A
    }
    bgra
}

// ─── VA-API Frame Decoder (Intel/AMD hardware decode on Linux) ────────

#[cfg(feature = "vaapi")]
mod vaapi {
    use super::DecodedFrame;
    use ffmpeg_sys_next as ff;
    use log::{info, warn};
    use std::ptr;

    /// VA-API hardware-accelerated H264 decoder using FFmpeg hwaccel.
    ///
    /// Following RustDesk's approach: simple CPU readback pipeline.
    ///   H264 → VA-API decode (GPU) → av_hwframe_transfer_data (CPU readback)
    ///   → sws_scale YUV→BGRA → DecodedFrame
    ///
    /// No DMA-BUF, no EGL, no GPU rendering — just CPU readback.
    pub struct VaapiFrameDecoder {
        codec_ctx: *mut ff::AVCodecContext,
        hw_device_ctx: *mut ff::AVBufferRef,
        sws_ctx: *mut ff::SwsContext,
        /// Hardware pixel format (e.g. AV_PIX_FMT_VAAPI).
        hw_pix_fmt: ff::AVPixelFormat,
        has_decoded_frame: bool,
        /// Whether VA-API hwaccel is active.  When false, the codec
        /// context has no hw_device_ctx and operates in pure software.
        hw_accel_active: bool,
        /// Consecutive decode failures; used to auto-disable hwaccel.
        consecutive_failures: u32,
    }

    // SAFETY: VaapiFrameDecoder is only used from the single decode thread.
    unsafe impl Send for VaapiFrameDecoder {}

    impl VaapiFrameDecoder {
        /// Create a new VA-API decoder.
        ///
        /// Creates a VA-API device context directly from /dev/dri/renderD128,
        /// finds the h264 decoder with VA-API hwaccel, and opens the codec.
        pub fn new() -> Result<Self, String> {
            unsafe {
                let device_path = std::ffi::CString::new("/dev/dri/renderD128")
                    .map_err(|e| format!("CString error: {}", e))?;

                // Create VA-API device context directly
                let mut hw_device_ctx: *mut ff::AVBufferRef = ptr::null_mut();
                let ret = ff::av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                    device_path.as_ptr(),
                    ptr::null_mut(),
                    0,
                );
                if ret < 0 {
                    return Err(format!(
                        "av_hwdevice_ctx_create(VAAPI) failed: {}",
                        av_error_to_string(ret)
                    ));
                }
                info!("[VA-API] Device context created");

                // Find h264 decoder + VA-API hwaccel config
                let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_H264);
                if codec.is_null() {
                    ff::av_buffer_unref(&mut hw_device_ctx as *mut _);
                    return Err("h264 decoder not found in FFmpeg".into());
                }

                let mut hw_pix_fmt: ff::AVPixelFormat =
                    ff::AVPixelFormat::AV_PIX_FMT_NONE;
                for i in 0.. {
                    let cfg = ff::avcodec_get_hw_config(codec, i);
                    if cfg.is_null() {
                        break;
                    }
                    if (*cfg).methods & ff::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                        && (*cfg).device_type == ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
                    {
                        hw_pix_fmt = (*cfg).pix_fmt;
                        break;
                    }
                }
                if hw_pix_fmt == ff::AVPixelFormat::AV_PIX_FMT_NONE {
                    ff::av_buffer_unref(&mut hw_device_ctx as *mut _);
                    return Err("h264 decoder has no VA-API hwaccel config".into());
                }

                // Create + open codec context
                let mut codec_ctx = ff::avcodec_alloc_context3(codec);
                if codec_ctx.is_null() {
                    ff::av_buffer_unref(&mut hw_device_ctx as *mut _);
                    return Err("avcodec_alloc_context3 failed".into());
                }
                (*codec_ctx).hw_device_ctx = ff::av_buffer_ref(hw_device_ctx);

                let ret = ff::avcodec_open2(codec_ctx, codec, ptr::null_mut());
                if ret < 0 {
                    let err = av_error_to_string(ret);
                    ff::avcodec_free_context(&mut codec_ctx as *mut _);
                    ff::av_buffer_unref(&mut hw_device_ctx as *mut _);
                    return Err(format!("avcodec_open2(h264+vaapi) failed: {}", err));
                }

                info!("[VA-API] Decoder initialized (hwaccel=VAAPI, DMA-BUF=false)");

                Ok(Self {
                    codec_ctx,
                    hw_device_ctx,
                    sws_ctx: ptr::null_mut(),
                    hw_pix_fmt,
                    has_decoded_frame: false,
                    hw_accel_active: true,
                    consecutive_failures: 0,
                })
            }
        }

        /// Decode a complete H264 Annex-B frame.
        ///
        /// Pipeline: VA-API decode → CPU readback → sws_scale YUV→BGRA
        ///
        /// If VA-API hwaccel fails repeatedly, automatically disables
        /// hwaccel and falls back to pure software decoding.
        pub fn decode(&mut self, h264_data: &[u8]) -> (Option<DecodedFrame>, bool) {
            unsafe {
                // Create AVPacket with the H264 data
                let mut pkt = ff::av_packet_alloc();
                if pkt.is_null() {
                    return (None, !self.has_decoded_frame);
                }
                let ret = ff::av_new_packet(pkt, h264_data.len() as i32);
                if ret < 0 {
                    ff::av_packet_free(&mut pkt);
                    return (None, !self.has_decoded_frame);
                }
                ptr::copy_nonoverlapping(
                    h264_data.as_ptr(),
                    (*pkt).data,
                    h264_data.len(),
                );

                // Send packet to decoder
                let ret = ff::avcodec_send_packet(self.codec_ctx, pkt);
                ff::av_packet_free(&mut pkt);
                if ret < 0 && ret != -(libc::EAGAIN as i32) {
                    if self.has_decoded_frame {
                        warn!("[VA-API] send_packet error: {}", av_error_to_string(ret));
                    }
                    self.on_decode_failure();
                    return (None, !self.has_decoded_frame);
                }

                // Receive decoded frame
                let mut frame = ff::av_frame_alloc();
                let ret = ff::avcodec_receive_frame(self.codec_ctx, frame);
                if ret < 0 {
                    ff::av_frame_free(&mut frame);
                    if ret == -(libc::EAGAIN as i32) || ret == ff::AVERROR_EOF {
                        return (None, false);
                    }
                    if self.has_decoded_frame {
                        warn!("[VA-API] receive_frame error: {}", av_error_to_string(ret));
                    }
                    self.on_decode_failure();
                    return (None, !self.has_decoded_frame);
                }

                // Reset failure counter on success
                self.consecutive_failures = 0;

                let width = (*frame).width as u32;
                let height = (*frame).height as u32;

                // If frame is in hardware (VA-API) memory, transfer to CPU
                if self.hw_accel_active && (*frame).format == self.hw_pix_fmt as i32 {
                    let mut sw_frame = ff::av_frame_alloc();
                    let ret = ff::av_hwframe_transfer_data(sw_frame, frame, 0);
                    if ret < 0 {
                        ff::av_frame_free(&mut frame);
                        ff::av_frame_free(&mut sw_frame);
                        warn!("[VA-API] hwframe_transfer failed: {}", av_error_to_string(ret));
                        return (None, false);
                    }
                    ff::av_frame_copy_props(sw_frame, frame);

                    let pix_fmt = std::mem::transmute::<i32, ff::AVPixelFormat>((*sw_frame).format);
                    let bgra = self.convert_to_bgra(sw_frame, width, height, pix_fmt);

                    ff::av_frame_free(&mut sw_frame);
                    ff::av_frame_free(&mut frame);

                    match bgra {
                        Some(bgra_data) => {
                            self.has_decoded_frame = true;
                            return (Some(DecodedFrame {
                                bgra_data,
                                width,
                                height,
                            }), false);
                        }
                        None => {
                            warn!("[VA-API] YUV→BGRA conversion failed");
                        }
                    }
                    return (None, !self.has_decoded_frame);
                }

                // Frame is in CPU memory (software decode or hwaccel fallback)
                let pix_fmt = std::mem::transmute::<i32, ff::AVPixelFormat>((*frame).format);
                let bgra = self.convert_to_bgra(frame, width, height, pix_fmt);
                ff::av_frame_free(&mut frame);

                match bgra {
                    Some(bgra_data) => {
                        self.has_decoded_frame = true;
                        return (Some(DecodedFrame {
                            bgra_data,
                            width,
                            height,
                        }), false);
                    }
                    None => {
                        warn!("[VA-API] YUV→BGRA conversion failed");
                    }
                }

                (None, !self.has_decoded_frame)
            }
        }

        /// Called when a decode fails. After too many consecutive
        /// failures, disables VA-API hwaccel and re-opens the codec
        /// in pure software mode.
        fn on_decode_failure(&mut self) {
            self.consecutive_failures += 1;
            if self.hw_accel_active && self.consecutive_failures >= 5 {
                warn!("[VA-API] Too many failures, disabling hwaccel → software-only");
                self.switch_to_software();
            }
        }

        /// Re-open the codec without hw_device_ctx for pure software decoding.
        fn switch_to_software(&mut self) {
            unsafe {
                let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_H264);
                if codec.is_null() {
                    return;
                }
                // Close old context
                ff::avcodec_free_context(&mut self.codec_ctx);

                // Create new context WITHOUT hw_device_ctx
                self.codec_ctx = ff::avcodec_alloc_context3(codec);
                if self.codec_ctx.is_null() {
                    return;
                }
                // Do NOT set hw_device_ctx — pure software decoding
                let ret = ff::avcodec_open2(self.codec_ctx, codec, ptr::null_mut());
                if ret < 0 {
                    warn!("[VA-API] Software codec open failed: {}", av_error_to_string(ret));
                    return;
                }
                self.hw_accel_active = false;
                self.consecutive_failures = 0;
                info!("[VA-API] Switched to software-only decoding");
            }
        }

        /// Convert a decoded AVFrame to BGRA using sws_scale.
        unsafe fn convert_to_bgra(
            &mut self,
            frame: *mut ff::AVFrame,
            width: u32,
            height: u32,
            pix_fmt: ff::AVPixelFormat,
        ) -> Option<Vec<u8>> {
            self.sws_ctx = ff::sws_getCachedContext(
                self.sws_ctx,
                width as i32,
                height as i32,
                pix_fmt,
                width as i32,
                height as i32,
                ff::AVPixelFormat::AV_PIX_FMT_BGRA,
                ff::SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if self.sws_ctx.is_null() {
                return None;
            }

            // Allocate BGRA frame
            let mut bgra_frame: ff::AVFrame = std::mem::zeroed();
            bgra_frame.width = width as i32;
            bgra_frame.height = height as i32;
            bgra_frame.format = ff::AVPixelFormat::AV_PIX_FMT_BGRA as i32;
            let ret = ff::av_frame_get_buffer(&mut bgra_frame, 0);
            if ret < 0 {
                return None;
            }

            // sws_scale wants *const *const u8 for srcSlice
            let src_slices: *const *const u8 =
                (*frame).data.as_ptr() as *const *const u8;
            let src_strides: *const libc::c_int = (*frame).linesize.as_ptr();
            let dst_slices: *const *mut u8 =
                bgra_frame.data.as_ptr() as *const *mut u8;
            let dst_strides: *const libc::c_int = bgra_frame.linesize.as_ptr();

            // Convert
            ff::sws_scale(
                self.sws_ctx,
                src_slices,
                src_strides,
                0,
                height as i32,
                dst_slices,
                dst_strides,
            );

            // Copy BGRA data (handle linesize padding)
            let linesize = bgra_frame.linesize[0] as usize;
            let w4 = (width * 4) as usize;
            let mut bgra = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height as usize {
                let row_ptr = bgra_frame.data[0].add(y * linesize);
                let row = std::slice::from_raw_parts(row_ptr, w4);
                bgra.extend_from_slice(row);
            }

            ff::av_frame_unref(&mut bgra_frame);
            Some(bgra)
        }

        /// Flush the decoder (signal data loss).
        pub fn flush(&mut self) {
            unsafe {
                ff::avcodec_flush_buffers(self.codec_ctx);
            }
        }
    }

    impl Drop for VaapiFrameDecoder {
        fn drop(&mut self) {
            unsafe {
                if !self.sws_ctx.is_null() {
                    ff::sws_freeContext(self.sws_ctx);
                }
                if !self.codec_ctx.is_null() {
                    ff::avcodec_free_context(&mut self.codec_ctx);
                }
                if !self.hw_device_ctx.is_null() {
                    ff::av_buffer_unref(&mut self.hw_device_ctx);
                }
            }
        }
    }

    /// Convert an FFmpeg error code to a human-readable string.
    fn av_error_to_string(err: i32) -> String {
        let mut buf = [0i8; 128];
        unsafe {
            ff::av_strerror(err, buf.as_mut_ptr(), buf.len());
            std::ffi::CStr::from_ptr(buf.as_ptr())
                .to_str()
                .unwrap_or("unknown error")
                .to_string()
        }
    }
}

#[cfg(feature = "vaapi")]
pub use vaapi::VaapiFrameDecoder;
