//! Tauri commands for the A2A (Agent-to-Agent) protocol demo.
//!
//! Exposes a single round-trip command that generates a fresh Ed25519 keypair,
//! builds a sample Offer envelope, signs it, verifies the signature, and returns
//! the signed envelope JSON together with a `verified` flag.

use air_rs::{
    envelope::{Envelope, MessageBody, Offer, Value},
    signing::{sign_envelope, verify_envelope},
};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::json;

/// Generate a fresh Ed25519 keypair, build a sample Offer envelope, sign it,
/// verify the signature, and return `{ envelope: <signed>, verified: true }`.
///
/// No persistent state is modified — this is a stateless demo / smoke-test
/// command. A real send-message command would accept `to_did` + payload args
/// and use the agent's persisted signing key (see `commands/identity.rs`).
#[tauri::command]
pub async fn a2a_demo_round_trip() -> Result<serde_json::Value, String> {
    // 1. Fresh ephemeral keypair — demo only; production uses the persisted key.
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // 2. Build a sample Offer envelope (unsigned).
    let envelope = Envelope {
        id: uuid_v4(),
        from: "did:wba:bossclaw.ai:test-sender".to_string(),
        to: "did:wba:bossclaw.ai:test-recipient".to_string(),
        timestamp: utc_now_iso8601(),
        in_reply_to: None,
        thread_id: uuid_v4(),
        nonce: uuid_v4(),
        body: MessageBody::Offer(Offer {
            item_id: "bossclaw-demo-item-001".to_string(),
            offered_value: Value::Cash {
                amount_cents: 1000,
                currency: "USD".to_string(),
            },
            note: Some("Demo round-trip from Tauri".to_string()),
        }),
        signature: None,
    };

    // 3. Sign — consumes the envelope and returns a new one with `signature` set.
    let signed = sign_envelope(envelope, &signing_key).map_err(|e| e.to_string())?;

    // 4. Verify — errors are mapped to a String so Tauri propagates them to TS.
    verify_envelope(&signed, &verifying_key).map_err(|e| e.to_string())?;

    // 5. Return the signed envelope JSON alongside a boolean proof of verification.
    let envelope_value =
        serde_json::to_value(&signed).map_err(|e| format!("serialise: {e}"))?;

    Ok(json!({
        "envelope": envelope_value,
        "verified": true
    }))
}

/// Minimal UUID v4 generator using rand_core (already a workspace dep).
/// Not RFC 4122-compliant for variant/version bits, but sufficient for demo
/// nonces. Real code should use the `uuid` crate.
fn uuid_v4() -> String {
    use rand_core::RngCore;
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    // Set version (4) and variant (RFC 4122) bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        u16::from_be_bytes(bytes[4..6].try_into().unwrap()),
        u16::from_be_bytes(bytes[6..8].try_into().unwrap()),
        u16::from_be_bytes(bytes[8..10].try_into().unwrap()),
        {
            let mut tail = [0u8; 8];
            tail[2..8].copy_from_slice(&bytes[10..16]);
            u64::from_be_bytes(tail)
        },
    )
}

/// Returns the current UTC time as an ISO 8601 string at second precision,
/// e.g. `"2026-05-28T12:34:56Z"`.
///
/// Uses `std::time::SystemTime`; no external chrono dependency needed for
/// this demo. Callers may replace this with a proper datetime crate.
fn utc_now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert Unix epoch seconds to a naive ISO 8601 datetime (UTC).
    let s = secs % 60;
    let total_min = secs / 60;
    let m = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let total_days = total_hr / 24;
    // Days since 1970-01-01 to calendar date (Gregorian proleptic).
    let (year, month, day) = days_to_ymd(total_days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Convert a count of days since Unix epoch (1970-01-01) to a (year, month, day)
/// Gregorian tuple. Adapted from the classic algorithm; handles dates up to 9999.
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    // Shift epoch from 1970-01-01 to the Julian day number of 1970-01-01.
    let jd = days as u32 + 2_440_588;
    let p = jd + 68_569;
    let q = 4 * p / 146_097;
    let r = p - (146_097 * q).div_ceil(4);
    let s = 4000 * (r + 1) / 1_461_001;
    let t = r - 1461 * s / 4 + 31;
    let u = 80 * t / 2447;
    let day = t - 2447 * u / 80;
    let v = u / 11;
    let month = u + 2 - 12 * v;
    let year = 100 * (q - 49) + s + v;
    (year, month, day)
}
