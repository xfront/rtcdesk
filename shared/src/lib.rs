//! RTCDesk common / shared modules.
//!
//! Pure protocol definitions, wire format constants, and frame utilities.
//! Used by both the host (server) and viewer (client/Tauri).

pub mod auth;
pub mod frame;
pub mod signal;
pub mod local_id;
pub mod streaming;