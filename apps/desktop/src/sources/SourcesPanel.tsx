import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  pickFolder, addGrant, revokeGrant, listGrants, runIngest, listFiles,
  type GrantDto, type FileRecordDto, type IngestReportDto,
} from "../api/engine";
import { activeGrants } from "./grants";
import { ingestSummary } from "./ingestSummary";

export function SourcesPanel() {
  const [grants, setGrants] = useState<GrantDto[]>([]);
  const [files, setFiles] = useState<FileRecordDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [summary, setSummary] = useState<IngestReportDto | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [g, f] = await Promise.all([listGrants(), listFiles()]);
      setGrants(g);
      setFiles(f);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  useEffect(() => { void refresh(); }, []);

  if (unavailable) {
    return <p style={{ color: "#666" }}>Couldn’t reach the memory engine.</p>;
  }

  const onAdd = async () => {
    const path = await pickFolder();
    if (!path) return;
    await addGrant(path);
    await refresh();
  };
  const onRevoke = async (path: string) => { await revokeGrant(path); await refresh(); };
  const onIngest = async () => {
    setBusy(true);
    setIngestError(null);
    try {
      setSummary(await runIngest());
      await refresh();
    } catch (e) {
      setIngestError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const active = activeGrants(grants);

  return (
    <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid #eee" }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Sources</div>
      <p style={{ color: "#666", fontSize: 13 }}>
        Folders the agent may read into its memory. Files are read locally and never leave your machine.
      </p>

      <div style={{ display: "flex", gap: 8, margin: "8px 0" }}>
        <Button variant="secondary" onClick={onAdd}>Add folder</Button>
        <Button variant="primary" onClick={onIngest} disabled={busy || active.length === 0}>
          {busy ? "Ingesting…" : "Ingest now"}
        </Button>
      </div>

      {summary ? <p style={{ fontSize: 13 }}>{ingestSummary(summary)}</p> : null}
      {ingestError ? <p style={{ fontSize: 13, color: "#b00" }}>{ingestError}</p> : null}

      <ul style={{ paddingLeft: 18, fontSize: 13 }}>
        {active.map((g) => (
          <li key={g.canonical_root} style={{ marginBottom: 4 }}>
            <code>{g.canonical_root}</code>{" "}
            <button onClick={() => onRevoke(g.canonical_root)} style={{ marginLeft: 8 }}>Revoke</button>
          </li>
        ))}
        {active.length === 0 ? <li style={{ color: "#666", listStyle: "none" }}>No folders yet.</li> : null}
      </ul>

      {files.length > 0 ? (
        <details style={{ fontSize: 13 }}>
          <summary>{files.length} ingested file{files.length === 1 ? "" : "s"}</summary>
          <ul style={{ paddingLeft: 18 }}>
            {files.map((f) => <li key={f.file_event_id}><code>{f.canonical_path}</code></li>)}
          </ul>
        </details>
      ) : null}
    </div>
  );
}
