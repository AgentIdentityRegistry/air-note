/**
 * AIPanel — shows the AI-reply loop's drafted/sent replies for the SELECTED conversation.
 *
 * Presentational only: it reads from `useAiLoop()` and wires the three actions
 * (`approveSend`, `edit`, `discard`). No business logic, no direct Tauri calls.
 *
 * Filtering: `ReplyView.peer` is the sender DID, so filtering `replies` by the
 * currently-selected peer DID gives only the drafts relevant to this conversation.
 */

import { useState } from "react";
import { Button } from "../components/Button";
import { StatusBadge } from "../components/ui/StatusBadge";
import { useAiLoop } from "../state/aiLoop";

/** The peer DID of the currently-selected conversation (null → nothing selected). */
type Props = { selectedPeer: string | null };

export function AIPanel({ selectedPeer }: Props) {
  const { mode, replies, approveSend, edit, discard } = useAiLoop();

  // ── disabled mode: no reply model configured ──────────────────────────────
  if (mode === "disabled") {
    return (
      <div style={wrapStyle}>
        <StatusBadge tone="neutral">AI replies</StatusBadge>
        <span style={{ fontSize: 13, color: "var(--text-tertiary)", marginLeft: 8 }}>
          configure a reply model to enable AI drafts
        </span>
      </div>
    );
  }

  // ── loading or no conversation selected: nothing to show ──────────────────
  if (mode === "loading" || !selectedPeer) return null;

  // ── filter to replies for this conversation only ──────────────────────────
  const rows = [...replies.entries()].filter(([, v]) => v.peer === selectedPeer);
  if (rows.length === 0) return null;

  return (
    <div style={wrapStyle}>
      <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 4, display: "block" }}>
        AI drafts
      </span>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {rows.map(([envelopeId, view]) => (
          <ReplyRow
            key={envelopeId}
            envelopeId={envelopeId}
            view={view}
            approveSend={approveSend}
            edit={edit}
            discard={discard}
          />
        ))}
      </div>
    </div>
  );
}

// ── per-row component ──────────────────────────────────────────────────────────────────────────

type RowProps = {
  envelopeId: string;
  view: ReturnType<typeof useAiLoop>["replies"] extends Map<string, infer V> ? V : never;
  approveSend: (id: string) => Promise<void>;
  edit: (id: string, text: string) => void;
  discard: (id: string) => void;
};

function ReplyRow({ envelopeId, view, approveSend, edit, discard }: RowProps) {
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState(view.text);

  const handleEdit = () => {
    setEditText(view.text);
    setEditing(true);
  };
  const handleEditSave = () => {
    edit(envelopeId, editText);
    setEditing(false);
  };
  const handleEditCancel = () => {
    setEditing(false);
  };

  // ── drafting: stream in progress ────────────────────────────────────────
  if (view.status === "drafting") {
    return (
      <div style={rowStyle}>
        <StatusBadge tone="neutral">drafting…</StatusBadge>
        <p style={draftTextStyle}>{view.text || <em style={{ color: "var(--text-tertiary)" }}>generating…</em>}</p>
      </div>
    );
  }

  // ── drafted: human-reviewable draft ─────────────────────────────────────
  if (view.status === "drafted") {
    return (
      <div style={rowStyle}>
        <div style={{ display: "flex", gap: 4, alignItems: "center", flexWrap: "wrap" }}>
          <StatusBadge tone="primary">draft</StatusBadge>
          {view.reason ? (
            <StatusBadge tone="warning">{view.reason}</StatusBadge>
          ) : null}
        </div>

        {editing ? (
          <div style={{ marginTop: 6 }}>
            <textarea
              value={editText}
              onChange={(e) => setEditText(e.target.value)}
              style={textareaStyle}
              rows={4}
            />
            <div style={{ display: "flex", gap: 6, marginTop: 6 }}>
              <Button variant="primary" onClick={handleEditSave}>Save</Button>
              <Button variant="secondary" onClick={handleEditCancel}>Cancel</Button>
            </div>
          </div>
        ) : (
          <>
            <p style={draftTextStyle}>{view.text}</p>
            <div style={{ display: "flex", gap: 6, marginTop: 6 }}>
              <Button variant="primary" onClick={() => approveSend(envelopeId)}>Send</Button>
              <Button variant="secondary" onClick={handleEdit}>Edit</Button>
              <Button variant="secondary" onClick={() => discard(envelopeId)}>Discard</Button>
            </div>
          </>
        )}
      </div>
    );
  }

  // ── auto-sent: sent by the AI automatically ──────────────────────────────
  if (view.status === "auto-sent") {
    return (
      <div style={rowStyle}>
        <StatusBadge tone="success">✓ auto-sent</StatusBadge>
        <p style={sentTextStyle}>{view.text}</p>
      </div>
    );
  }

  // ── sent: human-approved manual send ─────────────────────────────────────
  if (view.status === "sent") {
    return (
      <div style={rowStyle}>
        <StatusBadge tone="success">✓ sent</StatusBadge>
        <p style={sentTextStyle}>{view.text}</p>
      </div>
    );
  }

  // ── failed ────────────────────────────────────────────────────────────────
  if (view.status === "failed") {
    return (
      <div style={rowStyle}>
        <div style={{ display: "flex", gap: 4, alignItems: "center", flexWrap: "wrap" }}>
          {view.retryable ? (
            <StatusBadge tone="warning">failed — retryable</StatusBadge>
          ) : (
            <StatusBadge tone="error">failed</StatusBadge>
          )}
          {view.reason ? (
            <span style={{ fontSize: 12, color: "var(--error)" }}>{view.reason}</span>
          ) : null}
        </div>
        {/* Retry is only offered when retryable AND the row is still in `drafted` state.
            A failed auto-send (status:"failed", no token, no draft to resend) gets the
            reason label only — the user can compose a manual reply via the Composer. */}
        {view.retryable ? (
          <div style={{ marginTop: 6 }}>
            <Button
              variant="secondary"
              onClick={() => approveSend(envelopeId)}
              style={{ fontSize: 12, padding: "4px 10px" }}
            >
              Retry
            </Button>
          </div>
        ) : null}
      </div>
    );
  }

  return null;
}

// ── styles ─────────────────────────────────────────────────────────────────────────────────────

const wrapStyle: React.CSSProperties = {
  marginTop: 12,
  padding: "10px 12px",
  borderRadius: 8,
  border: "1px solid var(--border-soft)",
  background: "var(--surface-soft)",
};

const rowStyle: React.CSSProperties = {
  padding: "8px 10px",
  borderRadius: 6,
  background: "var(--surface)",
  border: "1px solid var(--border-soft)",
};

const draftTextStyle: React.CSSProperties = {
  margin: "6px 0 0",
  fontSize: 13,
  color: "var(--text-primary)",
  lineHeight: 1.5,
  whiteSpace: "pre-wrap",
};

const sentTextStyle: React.CSSProperties = {
  margin: "4px 0 0",
  fontSize: 12,
  color: "var(--text-tertiary)",
  lineHeight: 1.4,
  whiteSpace: "pre-wrap",
};

const textareaStyle: React.CSSProperties = {
  width: "100%",
  fontSize: 13,
  fontFamily: "inherit",
  padding: "6px 8px",
  borderRadius: 6,
  border: "1px solid var(--border-soft)",
  resize: "vertical",
  boxSizing: "border-box",
};
