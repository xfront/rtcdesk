//! Local device ID generation and formatting.
//!
//! Strategy (tiered):
//! 1. Linux: read first non-loopback MAC address from `/sys/class/net`, hash → 9 digits
//! 2. Fallback: hostname hash
//! 3. Final fallback: random (seeded by current time)
//!
//! The ID is always a 9-digit decimal string, displayed as "XXX XXX XXX".

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ─── Public API ─────────────────────────────────────────────────────────────

/// Generate a stable 9-digit numeric device ID.
pub fn generate_local_id() -> String {
    if let Some(id) = mac_based_id() {
        return id;
    }
    if let Some(id) = hostname_based_id() {
        return id;
    }
    random_9digit()
}

/// Format a raw 9-digit ID string into a human-readable "XXX XXX XXX" form.
pub fn format_local_id(id: &str) -> String {
    let digits: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    let len = digits.len();
    if len >= 9 {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else {
        digits
    }
}

// ─── MAC-based ID (Linux only) ───────────────────────────────────────────────

fn mac_based_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        let Ok(entries) = fs::read_dir("/sys/class/net") else {
            return None;
        };
        let mut macs: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" {
                continue;
            }
            let addr_path = format!("/sys/class/net/{}/address", name);
            if let Ok(mac) = fs::read_to_string(&addr_path) {
                let mac = mac.trim().to_string();
                if mac != "00:00:00:00:00:00" && !mac.is_empty() {
                    macs.push(mac);
                }
            }
        }
        macs.sort(); // deterministic order
        if let Some(mac) = macs.first() {
            return Some(hash_to_9digit(mac));
        }
    }
    None
}

// ─── Hostname-based ID ───────────────────────────────────────────────────────

fn hostname_based_id() -> Option<String> {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    if hostname.is_empty() {
        return None;
    }
    Some(hash_to_9digit(&hostname))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Hash an arbitrary string into a 9-digit decimal string (no leading zeros).
pub fn hash_to_9digit(input: &str) -> String {
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    let raw = h.finish();
    let n = 100_000_000u64 + (raw % 900_000_000u64);
    format!("{}", n)
}

fn random_9digit() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    let raw = h.finish();
    let n = 100_000_000u64 + (raw % 900_000_000u64);
    format!("{}", n)
}

/// Generate a short random hex string for ephemeral viewer IDs.
pub fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}
