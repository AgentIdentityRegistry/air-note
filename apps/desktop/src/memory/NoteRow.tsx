import { useState, type CSSProperties } from "react";
import { Button } from "../components/Button";
import { supersedeNote, type NoteDto } from "../api/engine";
import { formatDay } from "./format";

const textareaStyle: CSSProperties = {
  width: "100%",
  fontSize: 14,
  fontFamily: "inherit",
  lineHeight: 1.4,
  padding: "6px 8px",
  borderRadius: 6,
  border: "1px solid var(--border-soft)",
  resize: "vertical",
  boxSizing: "border-box",
};

const rowStyle: CSSProperties = { padding: "10px 0", borderBottom: "1px solid var(--border-soft)" };

/**
 * One note in the Library, with edit-in-place "Supersede" (spec §7b). Clicking Supersede swaps the
 * row for a textarea prefilled with the current text plus Save/Cancel — the app's AIPanel edit idiom.
 * Save writes a replacement via `supersedeNote` (the daemon makes the old note vanish and the new one
 * current), then asks the panel to re-list so the swap shows. Save is guarded: no blank (the engine
 * rejects it — A4) and no unchanged text (a pointless self-supersede). Edit state is local to the row;
 * the only thing crossing back to the panel is `onSaved`, the re-list trigger.
 */
export function NoteRow({ note, onSaved }: { note: NoteDto; onSaved: () => Promise<void> | void }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(note.text);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const startEdit = () => {
    setText(note.text);
    setError(null);
    setEditing(true);
  };
  const cancel = () => {
    setEditing(false);
    setError(null);
    setText(note.text);
  };

  const trimmed = text.trim();
  // Guard: the engine rejects blank text (A4), and re-superseding to the same text is a no-op.
  const canSave = trimmed !== "" && trimmed !== note.text.trim();

  const save = async () => {
    if (!canSave || saving) return;
    setSaving(true);
    setError(null);
    try {
      await supersedeNote(note.event_id, trimmed);
    } catch (e) {
      // Tauri rejects Result<_, String> with a BARE STRING — show it as-is, keep the editor open.
      setError(String(e));
      setSaving(false);
      return;
    }
    // Success: close the editor and re-list. The re-list replaces THIS row with a fresh one for the
    // new note, so nothing sets state here afterward (avoids a write to an unmounting component).
    setSaving(false);
    setEditing(false);
    await onSaved();
  };

  if (editing) {
    return (
      <li style={rowStyle}>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label="Edit note"
          rows={3}
          style={textareaStyle}
        />
        {error ? (
          <p style={{ fontSize: 13, color: "var(--error)", margin: "6px 0 0" }}>{error}</p>
        ) : null}
        <div style={{ display: "flex", gap: 8, marginTop: 6 }}>
          <Button variant="primary" disabled={!canSave || saving} onClick={() => void save()}>
            {saving ? "Saving…" : "Save"}
          </Button>
          <Button variant="secondary" disabled={saving} onClick={cancel}>Cancel</Button>
        </div>
      </li>
    );
  }

  return (
    <li style={rowStyle}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
        <div>
          <div style={{ fontSize: 14, lineHeight: 1.4 }}>{note.text}</div>
          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>{formatDay(note.created_at)}</div>
        </div>
        <div style={{ flexShrink: 0 }}>
          <Button variant="secondary" onClick={startEdit}>Supersede</Button>
        </div>
      </div>
    </li>
  );
}
