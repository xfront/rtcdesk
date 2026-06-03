//! H.264 video encoder using Cisco OpenH264.
//!
//! Converts RGBA frames → YUV420P → H.264 **Annex B** NAL units
//! (4-byte start code `00 00 00 01` prefix).
//!
//! webrtc-rs `TrackLocalStaticSample.write_sample()` expects Annex B
//! framing — NOT AVCC (length-prefixed).  The RTP packetizer inside
//! webrtc-rs strips the start codes and produces RFC-6184 RTP packets
//! from the raw NAL data, so we only need to supply proper Annex B.

use anyhow::Result;
use log::info;
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, UsageType};
use openh264::formats::{RgbaSliceU8, YUVBuffer};

/// H.264 video encoder backed by OpenH264.
pub struct VideoEncoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    /// Padded to even dimensions (YUV 4:2:0 requirement).
    enc_w: usize,
    enc_h: usize,
}

impl VideoEncoder {
    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        // YUV 4:2:0 requires even dimensions
        let enc_w = if width % 2 != 0 {
            (width + 1) as usize
        } else {
            width as usize
        };
        let enc_h = if height % 2 != 0 {
            (height + 1) as usize
        } else {
            height as usize
        };

        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .bitrate(BitRate::from_bps(2_000_000))
            .max_frame_rate(FrameRate::from_hz(fps as f32));

        let encoder =
            Encoder::with_api_config(openh264::OpenH264API::from_source(), config)?;

        info!(
            "VideoEncoder: OpenH264 {}x{} @ {} fps (source: {}x{})",
            enc_w, enc_h, fps, width, height,
        );

        Ok(Self {
            encoder,
            width,
            height,
            enc_w,
            enc_h,
        })
    }

    /// Encode an RGBA frame to H.264 **Annex B** bitstream.
    ///
    /// Each NAL unit is prefixed with the 4-byte start code `00 00 00 01`.
    /// The first encoded frame (IDR) always includes SPS and PPS NAL units
    /// in the output, so the receiver can initialise a decoder without any
    /// out-of-band parameter sets.
    ///
    /// This format is what webrtc-rs `TrackLocalStaticSample.write_sample()`
    /// expects — its internal RTP packetiser handles the conversion to
    /// RFC-6184 RTP packets.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<Vec<u8>> {
        let w = self.width as usize;
        let h = self.height as usize;

        let expected = w * h * 4;
        if rgba.len() != expected {
            anyhow::bail!(
                "encoder: expected {} bytes ({}x{}x4), got {}",
                expected,
                w,
                h,
                rgba.len()
            );
        }

        // Pad to even dimensions if necessary (YUV 4:2:0 requirement).
        let yuv = if self.enc_w != w || self.enc_h != h {
            let padded = pad_rgba(rgba, w, h, self.enc_w, self.enc_h);
            YUVBuffer::from_rgb_source(RgbaSliceU8::new(&padded, (self.enc_w, self.enc_h)))
        } else {
            YUVBuffer::from_rgb_source(RgbaSliceU8::new(rgba, (self.enc_w, self.enc_h)))
        };

        let bitstream = self.encoder.encode(&yuv)?;

        // Build Annex B output from encoded NAL units.
        //
        // OpenH264's `nal_unit()` may return data that already includes the
        // 4-byte Annex B start code (`00 00 00 01`).  We check each NAL and
        // only prepend a start code when it is missing so that the output is
        // always well-formed Annex B — exactly what webrtc-rs's RTP
        // packetiser expects.
        const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
        const START_CODE3: [u8; 3] = [0x00, 0x00, 0x01];
        let mut annexb = Vec::new();

        for l in 0..bitstream.num_layers() {
            if let Some(layer) = bitstream.layer(l) {
                for n in 0..layer.nal_count() {
                    if let Some(nal) = layer.nal_unit(n) {
                        if nal.is_empty() {
                            continue;
                        }
                        // Only add start code if the NAL data doesn't already
                        // start with one.  OpenH264 versions differ: some
                        // include start codes, some don't.
                        if !nal.starts_with(&START_CODE)
                            && !nal.starts_with(&START_CODE3)
                        {
                            annexb.extend_from_slice(&START_CODE);
                        }
                        annexb.extend_from_slice(nal);
                    }
                }
            }
        }

        Ok(annexb)
    }

    /// Force the next encoded frame to be an IDR (keyframe).
    ///
    /// Call this when the remote side requests a keyframe (e.g. via PLI /
    /// FIR RTCP feedback).  The next call to `encode()` will produce an
    /// IDR frame with SPS + PPS NAL units, which allows a newly-joined
    /// or recovering viewer to start decoding immediately.
    pub fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
        info!("VideoEncoder: forced IDR keyframe for next encode");
    }

    pub fn shutdown(&mut self) {
        info!("VideoEncoder: shut down");
    }
}

/// Pad RGBA data from (w, h) to (enc_w, enc_h) by duplicating the
/// last column / row.  Only called when at least one dimension is odd.
fn pad_rgba(rgba: &[u8], w: usize, h: usize, enc_w: usize, enc_h: usize) -> Vec<u8> {
    let mut padded = vec![0u8; enc_w * enc_h * 4];

    for y in 0..h {
        let src_off = y * w * 4;
        let dst_off = y * enc_w * 4;
        padded[dst_off..dst_off + w * 4]
            .copy_from_slice(&rgba[src_off..src_off + w * 4]);
        // Duplicate last pixel to fill extra columns
        for x in w..enc_w {
            let d = dst_off + x * 4;
            let s = dst_off + (w - 1) * 4;
            let (left, right) = padded.split_at_mut(d);
            right[..4].copy_from_slice(&left[s..s + 4]);
        }
    }
    // Duplicate last row to fill extra rows
    for y in h..enc_h {
        let dst_off = y * enc_w * 4;
        let src_off = (h - 1) * enc_w * 4;
        let (left, right) = padded.split_at_mut(dst_off);
        right[..enc_w * 4].copy_from_slice(&left[src_off..src_off + enc_w * 4]);
    }

    padded
}
