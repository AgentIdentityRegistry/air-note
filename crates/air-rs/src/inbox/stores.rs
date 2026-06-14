//! Read-only, home-scoped views of the CLI's JSON stores (contacts/blocklist/identity) plus the
//! mute set and the DID→AIR-id helper. Ports peers.mjs / contacts.mjs / moderation.mjs / identity.mjs.
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// Short AIR-id from a DID (ports `shortPeer`): first `AIR-<alnum/->` run, else the input.
pub fn short_peer(did: &str) -> String {
    let bytes = did.as_bytes();
    if let Some(start) = did.find("AIR-") {
        let mut end = start + 4;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_alphanumeric() || c == b'-' {
                end += 1
            } else {
                break;
            }
        }
        if end > start + 4 {
            return did[start..end].to_string();
        }
    }
    did.to_string()
}

/// Parse `AIRMSG_MUTE` (comma-separated alias/DID/AIR-id) into a set (ports `parseMuteSet`).
pub fn parse_mute_set() -> HashSet<String> {
    std::env::var("AIRMSG_MUTE")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A pinned contact (only the fields A2 consumes; serde ignores the rest).
#[derive(Debug, Clone, Deserialize)]
pub struct Contact {
    /// Display alias for the contact; truthy presence means "pinned".
    #[serde(default)]
    pub alias: Option<String>,
    /// The contact's AIR id, if recorded.
    #[serde(default)]
    pub air_id: Option<String>,
    /// The contact's pinned public key (multibase), if recorded.
    #[serde(default)]
    pub public_key_multibase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContactsFile {
    #[serde(default)]
    contacts: std::collections::HashMap<String, Contact>,
}

/// Current pinned contact for a DID (ports `getContactByDid`). None on any read/parse error.
pub fn get_contact_by_did(home: &Path, did: &str) -> Option<Contact> {
    let raw = std::fs::read_to_string(home.join("contacts.json")).ok()?;
    let file: ContactsFile = serde_json::from_str(&raw).ok()?;
    file.contacts.get(did).cloned()
}

/// Is this DID blocked (ports `isBlocked`)? **Fail-OPEN (false) on ANY error** — a corrupt
/// blocklist must never black-hole all mail (moderation.mjs D6).
pub fn is_blocked(home: &Path, did: &str) -> bool {
    (|| -> Option<bool> {
        let raw = std::fs::read_to_string(home.join("blocklist.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        Some(v.get("blocked")?.get(did).is_some())
    })()
    .unwrap_or(false)
}

/// The daemon identity METADATA the desktop is allowed to read (PUBLIC fields only).
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonIdentityMeta {
    /// The daemon identity's DID.
    pub did: String,
    /// Human-friendly name, if recorded.
    #[serde(default)]
    pub name: Option<String>,
}

/// Read `{home}/identity.json` and return ONLY did + name. SENSITIVE fields (`seed_hex`,
/// `agent_secret`) are never deserialized into the desktop process (design §4).
pub fn read_daemon_identity_meta(home: &Path) -> Option<DaemonIdentityMeta> {
    let raw = std::fs::read_to_string(home.join("identity.json")).ok()?;
    serde_json::from_str::<DaemonIdentityMeta>(&raw).ok()
}
