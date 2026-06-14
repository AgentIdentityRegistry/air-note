//! Identity adoption (design §4): one agent, many surfaces. If the daemon home has an identity, the
//! desktop ADOPTS it (did + name + DERIVED air_id) and reports any prior desktop-created identity as
//! dormant. `create_identity` MUST be disabled whenever a daemon home exists.
use crate::inbox::stores::{read_daemon_identity_meta, short_peer};
use serde::Serialize;
use std::path::Path;

/// The result of an adoption decision.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Adoption {
    /// The daemon identity is adopted. `dormant_did` is the desktop's prior self-created identity,
    /// if any (shown once as "now dormant").
    Adopted {
        /// The adopted DID.
        did: String,
        /// The AIR id derived from the DID.
        air_id: String,
        /// The adopted identity's name, if recorded.
        name: Option<String>,
        /// The desktop's prior self-created DID, now dormant (if any).
        dormant_did: Option<String>,
    },
    /// No daemon identity anywhere → the desktop shows the install-the-CLI screen. Identity
    /// creation on a fresh machine is OUT OF SCOPE v1.
    NeedsDaemon,
}

/// Decide adoption from the daemon home + the desktop's own prior identity DID (if it created one).
pub fn adopt(home: &Path, desktop_prior_did: Option<&str>) -> Adoption {
    match read_daemon_identity_meta(home) {
        Some(meta) => {
            let air_id = short_peer(&meta.did);
            let dormant = match desktop_prior_did {
                Some(d) if d != meta.did => Some(d.to_string()),
                _ => None,
            };
            Adoption::Adopted { did: meta.did, air_id, name: meta.name, dormant_did: dormant }
        }
        None => Adoption::NeedsDaemon,
    }
}

/// `create_identity` gate: forbidden whenever a daemon home identity exists (design §4 — a "reset"
/// must not re-fork the split-brain).
pub fn creation_allowed(home: &Path) -> bool {
    read_daemon_identity_meta(home).is_none()
}
