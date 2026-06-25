use super::types::*;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum AirError {
    #[error("AIR not reachable: {0}")]
    NotReachable(String),
    #[error("AIR returned an error: status={status} body={body}")]
    Api { status: u16, body: String },
    #[error("DID not found: {0}")]
    NotFound(String),
    #[error("auth failed (bad agent_secret)")]
    Unauthorized,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("other: {0}")]
    Other(String),
}

#[async_trait]
pub trait AirClient: Send + Sync {
    /// Register a new agent. Returns the registered record + one-time agent_secret.
    async fn register(
        &self,
        did: &Did,
        manifest: &AgentManifest,
    ) -> Result<RegistrationResponse, AirError>;

    /// Look up an agent record by DID.
    async fn lookup(&self, did: &Did) -> Result<AgentRecord, AirError>;

    /// Update an agent's manifest (requires per-agent secret).
    async fn update(
        &self,
        did: &Did,
        agent_secret: &str,
        manifest: &AgentManifest,
    ) -> Result<AgentRecord, AirError>;

    /// Check whether a username (raw, un-normalized) is valid + available. Unauthenticated.
    /// The registry always answers 200: see `UsernameCheck` for the valid vs. invalid shapes.
    async fn check_username(&self, username: &str) -> Result<UsernameCheck, AirError>;

    /// Claim a published unique `@handle` for an agent (requires per-agent secret).
    /// `Err(AirError::Conflict)` means taken or in cooldown; `Err(AirError::Unauthorized)`
    /// means a bad secret. Success carries no body — the registry returns only a summary.
    async fn claim_username(
        &self,
        did: &Did,
        agent_secret: &str,
        username: &str,
    ) -> Result<(), AirError>;

    /// Get trust score (may be derived from record.trust_score; cheap lookup).
    async fn trust_score(&self, did: &Did) -> Result<TrustScore, AirError>;

    /// Health check / "am I talking to AIR" probe.
    async fn health(&self) -> Result<(), AirError>;
}
