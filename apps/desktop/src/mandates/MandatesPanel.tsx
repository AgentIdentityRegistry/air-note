import { useEffect, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  pickFolder, setMandatesEnabled, mandatesEnabled as readMandatesEnabled, addMandate, revokeMandate,
  listMandates, mandateWrites, undoApply,
  type MandateDto, type MandateWriteDto,
} from "../api/engine";
import { validateMandateForm } from "./mandateForm";
import { toMandateRow, toActivityRow } from "./mandateView";

/** How often the active list + activity list refresh while the Mandates tab is open. */
const POLL_MS = 5000;

export function MandatesPanel() {
  const [enabled, setEnabled] = useState(false);
  const [mandates, setMandates] = useState<MandateDto[]>([]);
  const [writes, setWrites] = useState<MandateWriteDto[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // New-mandate form fields.
  const [target, setTarget] = useState("");
  const [sourceScope, setSourceScope] = useState("");
  const [recipe, setRecipe] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      // SF5: read the persisted mandates flag too, so the toggle reflects an explicit "on" after
      // relaunch (write-then-reflect alone would show OFF until clicked). Failures hide the toggle
      // state as off (the list reads below set `unavailable`).
      const [on, ms, ws] = await Promise.all([readMandatesEnabled(), listMandates(), mandateWrites()]);
      setEnabled(on);
      setMandates(ms);
      setWrites(ws);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onToggle = async (on: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await setMandatesEnabled(on);
      await refresh(); // re-read the persisted flag so the displayed state always matches the engine.
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onPickTarget = async () => {
    // The folder picker returns a directory; the user appends the file name in the field. (A
    // dedicated file picker is a fast-follow; for SP5 the path field is editable.)
    const dir = await pickFolder();
    if (dir) setTarget(dir.endsWith("/") ? dir : `${dir}/`);
  };
  const onPickScope = async () => {
    const dir = await pickFolder();
    if (dir) setSourceScope(dir);
  };

  const onCreate = async () => {
    const form = { target, sourceScope, recipe };
    const v = validateMandateForm(form);
    if (!v.ok) {
      setFormError(v.error);
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      await addMandate(target.trim(), sourceScope.trim(), recipe.trim());
      setTarget("");
      setSourceScope("");
      setRecipe("");
      await refresh();
    } catch (e) {
      // The engine's typed grant rejection stringifies to its bare reason (Rejected Display).
      setFormError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onRevoke = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await revokeMandate(id);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onUndo = async (fileWrittenId: string) => {
    setBusy(true);
    setError(null);
    try {
      // Undo reuses the SP4 engine undo (re-gated, hash-verified restore) — statically imported.
      await undoApply(fileWrittenId);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Mandates</h2>
        <p style={{ color: "#666" }}>Couldn’t reach the memory engine. Set up your identity first.</p>
      </Card>
    );
  }

  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Mandates</h2>
      {error ? <p style={{ color: "#b00", fontSize: 13 }}>{error}</p> : null}

      <Card>
        <label style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 14 }}>
          <input type="checkbox" checked={enabled} disabled={busy} onChange={(e) => void onToggle(e.target.checked)} />
          Mandates {enabled ? "on" : "off"} — when on, the brain keeps each mandate’s target file in
          sync and auto-applies clean changes (risky ones go to Review).
        </label>
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>New mandate</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input
              placeholder="Target file (in an edit-enabled folder)"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
            />
            <Button variant="secondary" disabled={busy} onClick={() => void onPickTarget()}>Pick folder…</Button>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input
              placeholder="Source folder (read-granted)"
              value={sourceScope}
              onChange={(e) => setSourceScope(e.target.value)}
              style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
            />
            <Button variant="secondary" disabled={busy} onClick={() => void onPickScope()}>Pick folder…</Button>
          </div>
          <textarea
            placeholder="Recipe: how to keep the target in sync from the sources"
            value={recipe}
            onChange={(e) => setRecipe(e.target.value)}
            rows={3}
            style={{ padding: 6, fontFamily: "inherit", fontSize: 13 }}
          />
          {formError ? <p style={{ color: "#b00", fontSize: 13, margin: 0 }}>{formError}</p> : null}
          <div>
            <Button variant="primary" disabled={busy} onClick={() => void onCreate()}>Create mandate</Button>
          </div>
        </div>
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Active mandates</div>
        {mandates.length === 0 ? (
          <p style={{ color: "#666", fontSize: 13 }}>No mandates yet.</p>
        ) : (
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {mandates.map((m) => {
              const row = toMandateRow(m);
              return (
                <li key={row.id} style={{ marginBottom: 8 }}>
                  <div><code>{row.targetName}</code> <span style={{ color: "#666" }}>in {row.targetFolder}</span></div>
                  <div style={{ color: "#666", fontSize: 12 }}>from <code>{row.sourceScope}</code></div>
                  <div style={{ fontSize: 12 }}>Recipe: {row.recipe}</div>
                  <button disabled={busy} onClick={() => void onRevoke(row.id)} style={{ marginTop: 4 }}>Revoke</button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Recent mandate activity</div>
        {writes.length === 0 ? (
          <p style={{ color: "#666", fontSize: 13 }}>No mandate changes yet.</p>
        ) : (
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {[...writes].reverse().map((w) => {
              const row = toActivityRow(w);
              return (
                <li key={row.fileWrittenId} style={{ marginBottom: 4 }}>
                  <code>{row.fileName}</code> <span style={{ color: "#666", fontSize: 12 }}>· {row.label}</span>{" "}
                  <button disabled={busy || !row.canUndo} onClick={() => void onUndo(row.fileWrittenId)} style={{ marginLeft: 8 }}>
                    Undo
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>
    </div>
  );
}
