import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { useIdentity } from "../state/identity";
import { renameIdentity, checkUsername, claimUsername, UsernameCheck } from "../api/tauri";
import { validateDisplayName } from "./displayName";
import { validateUsername } from "./username";

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

      <UsernameSection username={identity.username} onClaimed={refresh} />

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

/** The published unique @handle. Deliberately a SEPARATE section from the local-name rename:
 *  the rename edits `identity.name` (freely editable, local), while the @handle is published,
 *  globally unique, and claim-once (changing it later costs a 30-day cooldown). Flow: type a
 *  draft → Check availability → Claim (only when available). */
function UsernameSection({
  username,
  onClaimed,
}: {
  username: string | null;
  onClaimed: () => Promise<void>;
}) {
  const [draft, setDraft] = useState("");
  const [check, setCheck] = useState<UsernameCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [claiming, setClaiming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Already claimed → read-only with a cooldown note. The handle is claim-once for normal use.
  if (username) {
    return (
      <div style={{ marginTop: 16, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
        <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>Public username</div>
        <div style={{ fontSize: 16, fontWeight: 600 }}>@{username}</div>
        <div style={{ fontSize: 12, color: "var(--text-tertiary)", marginTop: 4 }}>
          This is your published unique handle. Changing it has a 30-day cooldown.
        </div>
      </div>
    );
  }

  const validation = validateUsername(draft);
  // Local charset/length validation gates the Check button so we don't ask the registry about
  // an obviously-invalid handle. A stale check is invalidated whenever the draft changes.
  const canCheck = !checking && validation.ok;
  const canClaim = !claiming && check?.valid === true && check.available;

  const onDraftChange = (value: string) => {
    setDraft(value);
    setCheck(null);
    setError(null);
  };

  const runCheck = async () => {
    if (!canCheck) return;
    setChecking(true);
    setError(null);
    setCheck(null);
    try {
      setCheck(await checkUsername(draft.trim()));
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const runClaim = async () => {
    if (!canClaim) return;
    setClaiming(true);
    setError(null);
    try {
      await claimUsername(draft.trim());
      await onClaimed();
      // After refresh the parent re-renders with `username` set, so this section flips to read mode.
    } catch (e) {
      // A 409 (taken / cooldown) arrives as a bare string here; show it inline.
      setError(String(e));
    } finally {
      setClaiming(false);
    }
  };

  return (
    <div style={{ marginTop: 16, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
      <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>Public username</div>
      <div style={{ fontSize: 12, color: "var(--text-tertiary)", marginBottom: 8 }}>
        Claim a unique, published @handle for your agent. This is different from the local name
        above, and can only be claimed once (changing it later has a 30-day cooldown).
      </div>
      <div className="chat-rename-wrap">
        <Input
          value={draft}
          disabled={claiming}
          placeholder="your_handle"
          aria-label="Public username"
          onChange={(e) => onDraftChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && canCheck) void runCheck();
          }}
        />
        <Button variant="secondary" disabled={!canCheck} onClick={() => void runCheck()}>
          {checking ? "Checking…" : "Check"}
        </Button>
        <Button disabled={!canClaim} onClick={() => void runClaim()}>
          {claiming ? "Claiming…" : "Claim"}
        </Button>
      </div>

      {/* Local validation feedback shows before the user has run a registry check. */}
      {draft !== "" && !validation.ok && !check ? (
        <div style={{ color: "var(--error)", fontSize: 13, marginTop: 6 }}>{validation.error}</div>
      ) : null}

      {check ? <CheckResult check={check} /> : null}

      {error ? (
        <div style={{ color: "var(--error)", fontSize: 13, marginTop: 6 }}>{error}</div>
      ) : null}
    </div>
  );
}

/** Renders the registry's verdict for a checked handle: available (green), or the
 *  taken/cooldown/invalid reason (red). */
function CheckResult({ check }: { check: UsernameCheck }) {
  if (check.valid && check.available) {
    return (
      <div style={{ color: "var(--success)", fontSize: 13, marginTop: 6 }}>
        @{check.username} is available.
      </div>
    );
  }
  // Not available (or invalid): prefer the registry's reason/error message.
  const message = !check.valid
    ? check.error ?? "That username isn’t valid."
    : check.reason === "cooldown"
      ? `@${check.username} is in a 30-day cooldown.`
      : `@${check.username} is already taken.`;
  return <div style={{ color: "var(--error)", fontSize: 13, marginTop: 6 }}>{message}</div>;
}
