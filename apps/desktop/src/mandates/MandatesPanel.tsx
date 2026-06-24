import { useEffect, useRef, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { ToggleSwitch } from "../components/ui/ToggleSwitch";
import {
  pickFolder, pickFile, setMandatesEnabled, mandatesEnabled as readMandatesEnabled, addMandate, revokeMandate,
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
  // Synchronous in-flight guard (mirrors ReviewPanel): `busy` state only updates after a
  // re-render, so a fast double-click would fire two engine calls before `disabled` takes
  // effect. This ref flips synchronously, so the second click is a true no-op. It also gates
  // the poll below — a poll landing mid-mutation could clobber the lists/toggle with
  // pre-mutation engine state (a stale `setInterval` closure can't read `busy` state, but it
  // can read this ref).
  const inFlight = useRef(false);

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
    // Skip a poll while a mutation is in flight so it can't clobber the lists/toggle with
    // pre-mutation engine state (the post-mutation `refresh()` in each handler repaints).
    const id = setInterval(() => {
      if (!inFlight.current) void refresh();
    }, POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onToggle = async (on: boolean) => {
    if (inFlight.current) return; // synchronous double-click guard (see `inFlight`).
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await setMandatesEnabled(on);
      await refresh(); // re-read the persisted flag so the displayed state always matches the engine.
    } catch (e) {
      setError(String(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  const onPickTarget = async () => {
    // The target IS a single file, so pick it directly (no folder + hand-typed filename).
    const file = await pickFile();
    if (file) setTarget(file);
  };
  const onPickScope = async () => {
    const dir = await pickFolder();
    if (dir) setSourceScope(dir);
  };

  const onCreate = async () => {
    if (inFlight.current) return; // synchronous double-click guard (see `inFlight`).
    const form = { target, sourceScope, recipe };
    const v = validateMandateForm(form);
    if (!v.ok) {
      setFormError(v.error);
      return;
    }
    inFlight.current = true;
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
      inFlight.current = false;
      setBusy(false);
    }
  };

  const onRevoke = async (id: string) => {
    if (inFlight.current) return; // synchronous double-click guard (see `inFlight`).
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await revokeMandate(id);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  const onUndo = async (fileWrittenId: string) => {
    if (inFlight.current) return; // synchronous double-click guard (see `inFlight`).
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      // Undo reuses the SP4 engine undo (re-gated, hash-verified restore) — statically imported.
      await undoApply(fileWrittenId);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Mandates</h2>
        <p style={{ color: "var(--text-secondary)" }}>Couldn’t reach your agent’s memory. Set up your identity first.</p>
      </Card>
    );
  }

  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Mandates</h2>
      <p style={{ color: "var(--text-secondary)", fontSize: 13, margin: "0 0 12px", lineHeight: 1.5 }}>
        A mandate is a standing rule: “keep this file of mine up to date from these folders.” Your agent
        watches the folders and proposes an edit for you to approve — it never rewrites the file on its own.
      </p>
      {error ? <p style={{ color: "var(--error)", fontSize: 13 }}>{error}</p> : null}

      <Card>
        <ToggleSwitch
          checked={enabled}
          disabled={busy}
          onChange={(next) => void onToggle(next)}
          label={
            <>
              Mandates are {enabled ? "on" : "off"} — when on, your agent keeps each file in sync and
              applies clean changes for you; anything risky waits for you in Review.
            </>
          }
        />
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>New Mandate</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 13, fontWeight: 500 }}>File to keep updated</span>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                placeholder="e.g. ~/Notes/team-roster.md"
                value={target}
                onChange={(e) => setTarget(e.target.value)}
                style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
              />
              <Button variant="secondary" disabled={busy} onClick={() => void onPickTarget()}>Pick File…</Button>
            </div>
            <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>Must be in a folder you’ve allowed edits on.</span>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 13, fontWeight: 500 }}>Folders to watch</span>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                placeholder="e.g. ~/Notes/people"
                value={sourceScope}
                onChange={(e) => setSourceScope(e.target.value)}
                style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
              />
              <Button variant="secondary" disabled={busy} onClick={() => void onPickScope()}>Pick Folder…</Button>
            </div>
            <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>Your agent reads these to keep the file current.</span>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 13, fontWeight: 500 }}>How to keep it in sync</span>
            <textarea
              placeholder="e.g. Keep the roster sorted by name; add anyone new I’ve written about."
              value={recipe}
              onChange={(e) => setRecipe(e.target.value)}
              rows={3}
              style={{ padding: 6, fontFamily: "inherit", fontSize: 13 }}
            />
          </div>
          {formError ? <p style={{ color: "var(--error)", fontSize: 13, margin: 0 }}>{formError}</p> : null}
          <div>
            <Button variant="primary" disabled={busy} onClick={() => void onCreate()}>Create Mandate</Button>
          </div>
        </div>
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Active Mandates</div>
        {mandates.length === 0 ? (
          <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>No mandates yet. Create one above to keep a file in sync.</p>
        ) : (
          <ul style={{ listStyle: "none", paddingLeft: 0, margin: 0, fontSize: 13 }}>
            {mandates.map((m) => {
              const row = toMandateRow(m);
              return (
                <li key={row.id} style={{ marginBottom: 12, paddingBottom: 12, borderBottom: "1px solid var(--border-soft)" }}>
                  <div><code>{row.targetName}</code> <span style={{ color: "var(--text-secondary)" }}>in {row.targetFolder}</span></div>
                  <div style={{ color: "var(--text-secondary)", fontSize: 12 }}>Watching <code>{row.sourceScope}</code></div>
                  <div style={{ fontSize: 12, color: "var(--text-secondary)" }}>How: {row.recipe}</div>
                  <button disabled={busy} onClick={() => void onRevoke(row.id)} style={{ marginTop: 6 }}>Remove</button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Recent Mandate Activity</div>
        {writes.length === 0 ? (
          <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>No changes yet. Edits your agent makes will show up here, with an Undo.</p>
        ) : (
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {[...writes].reverse().map((w) => {
              const row = toActivityRow(w);
              return (
                <li key={row.fileWrittenId} style={{ marginBottom: 4 }}>
                  <code>{row.fileName}</code> <span style={{ color: "var(--text-secondary)", fontSize: 12 }}>· {row.label}</span>{" "}
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
