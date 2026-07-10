//! Thin wire client: Hello/HelloOk once, then one Request → one Response per op (single
//! in-flight). `read_frame`/`write_frame` are NOT cancellation-safe, so the timeout wraps the
//! WHOLE op and a timed-out/error'd stream is DROPPED, never reused.

use std::path::Path;
use std::time::Duration;

use bossclawd_proto::types::{FileRecordMirror, IngestReportMirror};
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, HitWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

/// Per-op bound so a hung daemon can't wedge a multi-hour run. Ingest of ~866 pages through
/// the real embedder is minutes; 600s is ample headroom.
const OP_TIMEOUT: Duration = Duration::from_secs(600);

/// A connected wire client: one `UnixStream`, one op in flight at a time.
pub struct WireClient {
    stream: UnixStream,
    /// The frame protocol is not cancellation-safe, so a timed-out stream is permanently
    /// unusable — this flag turns accidental reuse into an instant named error instead of a
    /// framing desync (enforcing the pinned drop-on-error design).
    poisoned: bool,
}

impl WireClient {
    /// Connect + Hello/HelloOk handshake; verifies the protocol version.
    pub async fn connect(sock: &Path) -> anyhow::Result<Self> {
        let mut stream = UnixStream::connect(sock).await?;
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
        write_frame(&mut stream, &serde_json::to_vec(&hello)?).await?;
        let reply = read_frame(&mut stream).await?;
        let hello_ok: HelloOk = serde_json::from_slice(&reply)?;
        if hello_ok.proto_version != PROTO_VERSION {
            anyhow::bail!("daemon protocol {} != client {}", hello_ok.proto_version, PROTO_VERSION);
        }
        Ok(Self { stream, poisoned: false })
    }

    /// One Request → one Response, bounded by `OP_TIMEOUT`. On timeout the frame future is
    /// dropped mid-I/O — the stream is corrupt and MUST NOT be reused; the client poisons
    /// itself so any later call fails by name.
    async fn call(&mut self, req: Request) -> anyhow::Result<Response> {
        if self.poisoned {
            anyhow::bail!("wire client poisoned by an earlier timeout; reconnect required");
        }
        let fut = async {
            write_frame(&mut self.stream, &serde_json::to_vec(&req)?).await?;
            let frame = read_frame(&mut self.stream).await?;
            Ok::<Response, anyhow::Error>(serde_json::from_slice(&frame)?)
        };
        match tokio::time::timeout(OP_TIMEOUT, fut).await {
            Ok(r) => r,
            Err(_) => {
                self.poisoned = true;
                anyhow::bail!("wire op timed out after {OP_TIMEOUT:?}; stream is now unusable")
            }
        }
    }

    /// `AddGrant` (onboarded=true).
    pub async fn add_grant(&mut self, path: &Path) -> anyhow::Result<()> {
        match self.call(Request::AddGrant { onboarded: true, path: path.to_path_buf() }).await? {
            Response::Ok => Ok(()),
            Response::Err { kind, message } => anyhow::bail!("AddGrant failed: {kind:?}: {message}"),
            other => anyhow::bail!("AddGrant → unexpected {other:?}"),
        }
    }

    /// `RunIngest` (onboarded=true) → the ingest report.
    pub async fn run_ingest(&mut self) -> anyhow::Result<IngestReportMirror> {
        match self.call(Request::RunIngest { onboarded: true }).await? {
            Response::RunIngest(r) => Ok(r),
            Response::Err { kind, message } => anyhow::bail!("RunIngest failed: {kind:?}: {message}"),
            other => anyhow::bail!("RunIngest → unexpected {other:?}"),
        }
    }

    /// `ListFiles` (onboarded=true) → the current file records: the `event_id → page_id`
    /// bridge's source (spec §5 Rev 2).
    pub async fn list_files(&mut self) -> anyhow::Result<Vec<FileRecordMirror>> {
        match self.call(Request::ListFiles { onboarded: true }).await? {
            Response::ListFiles(files) => Ok(files),
            Response::Err { kind, message } => anyhow::bail!("ListFiles failed: {kind:?}: {message}"),
            other => anyhow::bail!("ListFiles → unexpected {other:?}"),
        }
    }

    /// `Recall` (onboarded=true) → the hydrated hits.
    pub async fn recall(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<HitWire>> {
        match self.call(Request::Recall { onboarded: true, query: query.to_string(), k }).await? {
            Response::Recall(hits) => Ok(hits),
            Response::Err { kind, message } => anyhow::bail!("Recall failed: {kind:?}: {message}"),
            other => anyhow::bail!("Recall → unexpected {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::HarnessDaemon;

    #[tokio::test]
    async fn grant_ingest_list_files_recall_over_wire() {
        // Mock embedder: PLUMBING TEST ONLY — quality numbers come from the live run with the
        // real embedder (spec §1 Rev 2).
        let d = HarnessDaemon::spawn_mock_for_plumbing_tests().unwrap();
        let corpus = d.home().join("corpus");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::write(corpus.join("a.md"), "ferris the crab loves rust").unwrap();

        let mut client = WireClient::connect(d.socket_path()).await.unwrap();
        client.add_grant(&corpus).await.unwrap();
        let report = client.run_ingest().await.unwrap();
        assert_eq!(report.ingested, 1, "one page ingested");

        // ListFiles: the event_id → canonical_path bridge's source (spec §5 Rev 2).
        let files = client.list_files().await.unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].canonical_path.ends_with("a.md"));
        assert!(!files[0].file_event_id.is_empty());

        let hits = client.recall("ferris crab", 5).await.unwrap();
        assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall hydrates the snippet");
    }
}
