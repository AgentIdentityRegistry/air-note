use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Did(pub String);

impl Did {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub did: Did,
    pub name: String,
    #[serde(default)]
    pub username: Option<String>,
    pub manifest_url: Option<String>,
    pub trust_score: TrustScore,
    pub created_at: String, // ISO 8601
    pub updated_at: String,
}

/// Result of `GET /agents/check-username`. The registry returns 200 in all cases:
/// a valid handle carries `available` + `reason` ("available"|"taken"|"cooldown");
/// an invalid handle carries `error` with a human message (and `valid=false`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsernameCheck {
    pub username: String,
    pub valid: bool,
    pub available: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TrustScore(pub u8); // 0-100

impl TrustScore {
    pub fn initial() -> Self {
        TrustScore(50)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>, // e.g. ["a2a-negotiate-marketplace", "sign-attestation"]
    pub owner_hint: Option<String>, // e.g. "human-controlled"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationResponse {
    pub did: Did,
    pub agent_secret: String, // returned once at registration
    pub record: AgentRecord,
}
