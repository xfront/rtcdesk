//! RTCDesk common / shared modules.
//!
//! Pure protocol definitions, wire format constants, and frame utilities.
//! Used by both the host (server) and viewer (client/Tauri).

pub mod auth;
pub mod frame;
pub mod local_id;
pub mod streaming;
pub mod tls;
pub mod ice0_service;
pub mod sdp_utils;