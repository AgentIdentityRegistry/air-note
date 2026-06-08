use super::client_trait::*;
use super::types::*;
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::json;

pub struct HttpAirClient {
    base_url: String,
    http: Client,
}

impl HttpAirClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: Client::new(),
        }
    }

    pub fn production() -> Self {
        Self::new("https://agentidentityregistry.org/api/v1")
    }

    async fn map_error(resp: reqwest::Response) -> AirError {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        match StatusCode::from_u16(status).ok() {
            Some(StatusCode::NOT_FOUND) => AirError::NotFound(body),
            Some(StatusCode::UNAUTHORIZED) | Some(StatusCode::FORBIDDEN) => AirError::Unauthorized,
            _ => AirError::Api { status, body },
        }
    }
}

#[async_trait]
impl AirClient for HttpAirClient {
    async fn register(
        &self,
        did: &Did,
        manifest: &AgentManifest,
    ) -> Result<RegistrationResponse, AirError> {
        let url = format!("{}/agents", self.base_url);
        let body = json!({
            "did": did.0,
            "manifest": manifest,
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AirError::NotReachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Self::map_error(resp).await);
        }
        resp.json::<RegistrationResponse>()
            .await
            .map_err(|e| AirError::Other(e.to_string()))
    }

    async fn lookup(&self, did: &Did) -> Result<AgentRecord, AirError> {
        let url = format!("{}/agents/{}", self.base_url, urlencoding::encode(&did.0));
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AirError::NotReachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Self::map_error(resp).await);
        }
        resp.json::<AgentRecord>()
            .await
            .map_err(|e| AirError::Other(e.to_string()))
    }

    async fn update(
        &self,
        did: &Did,
        agent_secret: &str,
        manifest: &AgentManifest,
    ) -> Result<AgentRecord, AirError> {
        let url = format!("{}/agents/{}", self.base_url, urlencoding::encode(&did.0));
        let body = json!({ "manifest": manifest });
        let resp = self
            .http
            .put(&url)
            .header("X-Agent-Secret", agent_secret)
            .json(&body)
            .send()
            .await
            .map_err(|e| AirError::NotReachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Self::map_error(resp).await);
        }
        resp.json::<AgentRecord>()
            .await
            .map_err(|e| AirError::Other(e.to_string()))
    }

    async fn trust_score(&self, did: &Did) -> Result<TrustScore, AirError> {
        // Reuse lookup; AIR's trust_score endpoint may differ — adjust when API lands
        let record = self.lookup(did).await?;
        Ok(record.trust_score)
    }

    async fn health(&self) -> Result<(), AirError> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AirError::NotReachable(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(Self::map_error(resp).await)
        }
    }
}
