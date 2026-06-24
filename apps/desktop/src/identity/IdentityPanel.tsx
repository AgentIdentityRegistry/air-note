import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { useIdentity } from "../state/identity";
import { renameIdentity } from "../api/tauri";
import { validateDisplayName } from "./displayName";

export function IdentityPanel() {
  const { identity, trustScore, loading, refresh } = useIdentity();
  // Rename-in-progress state. `draft` is the in-edit value; null `draft` means view mode.
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (loading) return <Card>Loading...</Card>;
  if (!identity) return <Card>No identity yet.</Card>;

  const editing = draft !== null;
  const validation = editing ? validateDisplayName(draft) : null;
  // Save is allowed only for a valid name that actually differs from the current one.
  const canSave =
    !saving && validation?.ok === true && validation.name !== identity.name;

  const startEdit = () => {
    setError(null);
    setDraft(identity.name);
  };
  const cancelEdit = () => {
    setError(null);
    setDraft(null);
  };
  const save = async () => {
    if (!canSave || draft === null) return;
    setSaving(true);
    setError(null);
    try {
      await renameIdentity(draft.trim());
      await refresh();
      setDraft(null);
    } catch (e) {
      // The command stringifies its validation/IO errors to a bare message.
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card>
      <h2 style={{ margin: 0 }}>Agent Identity Registry</h2>

      <div style={{ marginTop: 16 }}>
        <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>Name</div>
        {editing ? (
          <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}>
            <div className="chat-rename-wrap">
              <Input
                autoFocus
                value={draft}
                disabled={saving}
                aria-label="Display name"
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && canSave) void save();
                  if (e.key === "Escape") cancelEdit();
                }}
              />
              <Button disabled={!canSave} onClick={() => void save()}>
                Save
              </Button>
              <Button variant="secondary" disabled={saving} onClick={cancelEdit}>
                Cancel
              </Button>
            </div>
            {error ? <div style={{ color: "var(--error)", fontSize: 13 }}>{error}</div> : null}
          </div>
        ) : (
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <div style={{ fontSize: 16 }}>{identity.name}</div>
            <Button variant="secondary" aria-label="Rename" title="Rename" onClick={startEdit}>
              ✏️
            </Button>
          </div>
        )}
      </div>

      <div style={{ marginTop: 12 }}>
        <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>DID</div>
        <div
          style={{
            fontSize: 12,
            fontFamily: "monospace",
            wordBreak: "break-all",
          }}
        >
          {identity.did}
        </div>
      </div>

      <div style={{ marginTop: 12 }}>
        <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>Trust Score</div>
        <div style={{ fontSize: 16 }}>{trustScore ?? "—"}</div>
      </div>

      <div style={{ marginTop: 16 }}>
        <Button variant="secondary" onClick={refresh}>
          Refresh
        </Button>
      </div>
    </Card>
  );
}
