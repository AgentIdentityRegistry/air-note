import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  pickFolder, addGrant, revokeGrant, listGrants, runIngest, listFiles,
  setFolderWritable, setProposalsEnabled, setEvolveEnabled, evolveStatus, listWritable,
  type GrantDto, type FileRecordDto, type IngestReportDto,
} from "../api/engine";
import { activeGrants } from "./grants";
import { ingestSummary } from "./ingestSummary";
import { allWritable } from "./writableGrants";

export function SourcesPanel() {
  const [grants, setGrants] = useState<GrantDto[]>([]);
  const [files, setFiles] = useState<FileRecordDto[]>([]);
  const [writable, setWritable] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [summary, setSummary] = useState<IngestReportDto | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);
  const [evolveOn, setEvolveOn] = useState<boolean | null>(null);
  const [editError, setEditError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [g, f, ev, w] = await Promise.all([listGrants(), listFiles(), evolveStatus(), listWritable()]);
      setGrants(g);
      setFiles(f);
      setEvolveOn(ev.enabled);
      setWritable(new Set(w));
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  useEffect(() => { void refresh(); }, []);

  const active = activeGrants(grants);

  if (unavailable) {
    return <p style={{ color: "#666" }}>Couldn’t reach the memory engine.</p>;
  }

  const onAdd = async () => {
    const path = await pickFolder();
    if (!path) return;
    await addGrant(path);
    await refresh();
  };
  const onRevoke = async (path: string) => {
    setBusy(true);
    setEditError(null);
    try {
      await revokeGrant(path);
      await refresh();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  };
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
  const onToggleWritable = async (root: string, on: boolean) => {
    setBusy(true);
    setEditError(null);
    try {
      await setFolderWritable(root, on);
      if (on) await setProposalsEnabled(true); // Lock-1 enablement, under the hood.
      await refresh();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const onAllowAll = async (on: boolean) => {
    setBusy(true);
    setEditError(null);
    const failures: string[] = [];
    try {
      for (const g of active) {
        try {
          await setFolderWritable(g.canonical_root, on);
        } catch (e) {
          failures.push(`${g.canonical_root}: ${String(e)}`); // continue-and-report.
        }
      }
      if (on && failures.length < active.length) await setProposalsEnabled(true);
      if (failures.length > 0) setEditError(`Some folders failed: ${failures.join("; ")}`);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const onEnableEvolve = async () => {
    setBusy(true);
    setEditError(null);
    try {
      await setEvolveEnabled(true);
      await refresh();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const everyWritable = allWritable(active.map((g) => g.canonical_root), writable);

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
        <Button variant="secondary" onClick={() => void onAllowAll(!everyWritable)} disabled={busy || active.length === 0}>
          {everyWritable ? "Disallow all edits" : "Allow all edits"}
        </Button>
      </div>

      {summary ? <p style={{ fontSize: 13 }}>{ingestSummary(summary)}</p> : null}
      {ingestError ? <p style={{ fontSize: 13, color: "#b00" }}>{ingestError}</p> : null}
      {editError ? <p style={{ fontSize: 13, color: "#b00" }}>{editError}</p> : null}

      <ul style={{ paddingLeft: 18, fontSize: 13 }}>
        {active.map((g) => {
          const editable = writable.has(g.canonical_root);
          return (
            <li key={g.canonical_root} style={{ marginBottom: 4 }}>
              <code>{g.canonical_root}</code>{" "}
              <button disabled={busy} onClick={() => void onToggleWritable(g.canonical_root, !editable)} style={{ marginLeft: 8 }}>
                {editable ? "Disallow edits" : "Allow edits"}
              </button>
              <button disabled={busy} onClick={() => onRevoke(g.canonical_root)} style={{ marginLeft: 8 }}>Revoke</button>
            </li>
          );
        })}
        {active.length === 0 ? <li style={{ color: "#666", listStyle: "none" }}>No folders yet.</li> : null}
      </ul>

      {writable.size > 0 && evolveOn === false ? (
        <p style={{ fontSize: 13, color: "#b00" }}>
          Edits are enabled, but the learning loop is off, so no changes will be proposed.{" "}
          <button disabled={busy} onClick={() => void onEnableEvolve()}>Turn learning on</button>
        </p>
      ) : null}

      {writable.size > 0 ? (
        <p style={{ fontSize: 12, color: "#888" }}>
          Heads up: corrections the brain learned <em>before</em> you enabled a folder won’t be
          re-applied retroactively — only new contradictions are proposed.
        </p>
      ) : null}

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
