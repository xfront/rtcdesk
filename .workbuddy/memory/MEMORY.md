# rtcdesk Project Memory

## Architecture
- Host: Rust host-service (OpenH264 encoder → webrtc-rs RTP track)
- Client: Rust/Dioxus desktop app (WebSocket signaling → webrtc-rs PeerConnection → OpenH264 decode → RGB → JPEG → `<img>`)
- Signaling: WebSocket-based signaling server

## Video Pipeline (Pure Rust — current)
- Host: OpenH264 encoder → webrtc-rs RTP track, Annex B output, `force_intra_frame()` for IDR
- Client: webrtc-rs PeerConnection → `on_track` → H264 RTP depacketize → OpenH264 decode → RGB → JPEG → `<img data:URL>`
- No JS at all — no RTCPeerConnection, no ontrack, no video.srcObject
- Dual-task pipeline: fast task (`track.poll()` → `pkt_tx.send()`) + slow task (decode all packets, JPEG-encode only last frame)
- Stream decoding: each NAL unit goes to decoder immediately; marker triggers `flush_remaining()`

## Key Lessons (Pure Rust Client)
- **`on_track` fires during `set_remote_description`** — set `running=true` BEFORE that call, or spawned poll tasks exit immediately
- **H.264 frames span 30-60 RTP FU-A packets** — you CANNOT skip individual RTP packets; skipping even one corrupts the entire stream. Frame-level skipping (decode all, keep only last frame) is the only safe latency-control strategy.
- **Separate decode from encode** — OpenH264 decode to RGB is cheap (~3ms); JPEG encode is expensive (~40ms). Decode every packet, JPEG-encode only the last frame per drain cycle.
- **`unbounded_channel` causes multi-second latency** — slow task must drain to latest via `try_recv()` loop, but must feed every packet to decoder
- **`YUVSource` trait lacks `write_rgb8()`** — that method is on `DecodedYUV` struct; convert YUV→RGB inline in `process_rtp_payload`
- Host must break streaming loop on write_sample errors (client disconnection)
- webrtc-rs 0.20 alpha has NO RTCP callback in PeerConnectionEventHandler — must use periodic keyframes
