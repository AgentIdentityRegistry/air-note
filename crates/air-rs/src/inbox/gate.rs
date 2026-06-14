//! The channel admission gate (ports channel.mjs `channelGate`): verified + pinned (non-empty
//! contact alias) + key-unchanged + not-muted. Pure.
use crate::inbox::frames::Message;
use crate::inbox::stores::short_peer;
use std::collections::HashSet;

/// May this message be admitted to the AI channel? Verified + pinned (non-empty contact) +
/// key-unchanged + not muted by alias/DID/short-AIR-id. Pure; mirrors `channelGate`.
pub fn channel_gate(m: &Message, mute: &HashSet<String>) -> bool {
    let contact = match m.contact.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => return false,
    };
    if !m.verified || m.key_changed() {
        return false;
    }
    if mute.contains(contact) || mute.contains(&m.from) || mute.contains(&short_peer(&m.from)) {
        return false;
    }
    true
}
