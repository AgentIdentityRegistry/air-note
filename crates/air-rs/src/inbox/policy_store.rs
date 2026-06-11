//! `{home}/agent-policy.json` — the per-contact autonomy dial (design §7). Written ONLY by the
//! desktop (one writer per file; `contacts.json` is the CLI's). Corrupt/missing → all draft.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// Per-contact AI autonomy level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    /// The AI does nothing for this contact.
    Off,
    /// The AI drafts a reply for human review (the default).
    #[default]
    Draft,
    /// The AI may auto-send (subject to Phase B loop guards).
    Auto,
}

/// Policy for one contact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContactPolicy {
    /// The autonomy dial for this contact.
    #[serde(default)]
    pub ai_autonomy: Autonomy,
    /// Auto-sent envelope_ids (Phase B loop guard); A2 just round-trips it.
    #[serde(default)]
    pub auto_ledger: Vec<String>,
}

/// The whole policy file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Schema version.
    pub version: u32,
    /// Per-contact policies keyed by DID.
    #[serde(default)]
    pub contacts: HashMap<String, ContactPolicy>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { version: 1, contacts: HashMap::new() }
    }
}

fn policy_path(home: &Path) -> std::path::PathBuf {
    home.join("agent-policy.json")
}

/// Read the policy; a missing OR corrupt file yields the safe default (everything = draft).
pub fn load(home: &Path) -> Policy {
    std::fs::read_to_string(policy_path(home))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The dial for one contact — default `Draft` when absent (design §7).
pub fn autonomy_for(home: &Path, did: &str) -> Autonomy {
    load(home).contacts.get(did).map(|c| c.ai_autonomy).unwrap_or_default()
}

/// Set a contact's dial and persist 0600. Returns the written Policy.
pub fn set_autonomy(home: &Path, did: &str, value: Autonomy) -> std::io::Result<Policy> {
    let mut p = load(home);
    p.contacts.entry(did.to_string()).or_default().ai_autonomy = value;
    write_atomic(home, &p)?;
    Ok(p)
}

fn write_atomic(home: &Path, p: &Policy) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path = policy_path(home);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(p).expect("policy serializes");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.flush()?;
    }
    set_0600(&tmp);
    std::fs::rename(&tmp, &path)?;
    set_0600(&path);
    Ok(())
}

#[cfg(unix)]
fn set_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_0600(_path: &Path) {}
