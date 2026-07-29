import { useState, type CSSProperties } from "react";
import { Button } from "../components/Button";

/** One selected `remember` note — exported as a STAMPED item (its whole signed event). */
export type ExportNote = { id: string; text: string };
/** One selected captured session — exported as a SEAL_VOUCHED item (its full transcript). */
export type ExportSession = { id: string; title: string };
/** One selected ingested file — exported as a SEAL_VOUCHED item (its extracted text only). */
export type ExportIngest = { id: string; path: string };

/**
 * What each class actually ships, in the owner's words. These lines describe the REAL payload the
 * daemon builds (`EventLog::export_bundle`), not a summary of it: a stamped note travels as its
 * entire signed event, a session as its entire transcript. Showing the owner less than what leaves
 * their machine would make this sheet a lie.
 *
 * Both enumerating lines are coupled to core by frozen-key tests in `crates/bossclaw-core/src/log.rs`,
 * which name this constant in their failure messages. Do not edit one side without the other:
 * - `note` ↔ `a_stamped_note_discloses_exactly_these_fields` — the canonical event fields inside
 *   `event_bytes`. That test also pins that `signed_by_did` is the ENGINE placeholder, not the
 *   owner's did (`ENGINE_SIGNER_DID`, the M4/M7 gap): this line must not claim the owner's name is
 *   inside the note. When M4/M7 lands, that test fails and the clause gets restored.
 * - `session` ↔ `a_session_display_discloses_exactly_these_fields` — the six `display` fields.
 */
const DISCLOSURE = {
  note:
    "Ships the full signed record of this note: its exact text, when you saved it, its id, the link " +
    "to whatever you recorded just before it, and your brain’s signature. The receiver can check " +
    "that signature on its own.",
  session:
    "Ships the FULL transcript of this session, plus its title, which tool ran it, when it started " +
    "and ended, its rough size, and the project’s folder name — never the full path or the session " +
    "id in the file’s labels. The transcript itself may quote paths you or your tools typed.",
  ingest:
    "Ships the full extracted text of this file — content only. No path, no folder, no file hash " +
    "travels with it; the path below is only shown here so you can tell the files apart.",
} as const;

/**
 * The labels the RECEIVER's verifier prints for each class (`bossclaw-bundle`'s kind-aware
 * `verify_item`). Quoted verbatim so the owner sees, before sending, exactly how strong each item
 * will read on the other end — a stamped note carries the brain's signature, everything else is
 * only vouched for by the seal.
 */
const RECEIVER_LABEL = {
  note: "this brain recorded these bytes; provenance of the underlying text is not asserted",
  session: "captured session, content only; not independently verified",
  ingest: "ingested file extract; not independently verified",
} as const;

const listStyle: CSSProperties = { listStyle: "none", padding: 0, margin: 0 };
const rowStyle: CSSProperties = { padding: "8px 0", borderBottom: "1px solid var(--border-soft)" };
const identityStyle: CSSProperties = { fontSize: 13, lineHeight: 1.4, wordBreak: "break-word" };
const disclosureStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--text-secondary)",
  lineHeight: 1.5,
  margin: "2px 0 0",
};
const labelStyle: CSSProperties = {
  fontSize: 12,
  color: "var(--text-tertiary)",
  lineHeight: 1.5,
  margin: "2px 0 0",
};
const sectionHeadingStyle: CSSProperties = { fontSize: 13, margin: "12px 0 4px" };

/** One reviewed item: what it is, what ships, and how the receiver's verifier will label it. */
function ReviewRow({
  identity,
  disclosure,
  receiverLabel,
}: {
  identity: string;
  disclosure: string;
  receiverLabel: string;
}) {
  return (
    <li style={rowStyle}>
      <div style={identityStyle}>{identity}</div>
      <p style={disclosureStyle}>{disclosure}</p>
      <p style={labelStyle}>Your receiver’s verifier will label it: “{receiverLabel}”</p>
    </li>
  );
}

/**
 * The last thing the owner sees before a sealed `.airmem` leaves their machine: every selected
 * memory, what its bytes actually contain, and the two facts about sealing that are easy to get
 * backwards — it proves AUTHORSHIP, and it keeps NOTHING secret.
 *
 * Deliberately silent about identity. A sealed file proves this brain wrote these bytes; proving
 * WHICH registered identity that brain belongs to needs a registry lookup the receiver performs, so
 * this sheet never promises it (the same honesty rule the `air-verify` CLI follows at L1). This
 * component renders no verdict at all — it describes what is being written, and the receiver's
 * verifier is the only thing that judges a finished file.
 *
 * Tokens only (no hardcoded colors) per the shell-redesign gate.
 */
export function ExportReviewSheet({
  notes,
  sessions,
  ingests,
  exporting,
  error,
  onConfirm,
  onCancel,
}: {
  notes: ExportNote[];
  sessions: ExportSession[];
  ingests: ExportIngest[];
  exporting: boolean;
  error: string | null;
  onConfirm: (description: string) => void;
  onCancel: () => void;
}) {
  const [description, setDescription] = useState("");
  const total = notes.length + sessions.length + ingests.length;

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={() => { if (!exporting) onCancel(); }}>
      <div
        className="modal-card"
        role="dialog"
        aria-modal="true"
        aria-label="Review what you are about to export"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <h3>Send {total} {total === 1 ? "memory" : "memories"} as a signed file</h3>

        <p style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.5, margin: 0 }}>
          Anyone you send this file to can read the plaintext of everything selected — signing proves
          you wrote it, it does not keep it secret. Exporting saves one file to your Mac and does not
          publish anything.
        </p>

        {/* The identifier disclosure. THAT the name travels, in every file, is the owner's to know,
            and the linkability consequence is the part they would actually want warned about. What
            this must NOT do is promise the name proves anything: a bundle can name any did and
            still verify green offline (the branch's own `did_injection.airmem` vector), which is
            why `air-verify` prints "claimed by this file, NOT verified". Matching is something a
            receiver who ALREADY knows the name can do; checking it needs the registry (SP-V2). */}
        <p style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.5, margin: 0 }}>
          Every file also carries your AIR name (your did) and your public keys, so a receiver who
          already knows your AIR name can match it against the file, and so two receivers can tell
          two files came from the same person. Offline, that name is a claim they cannot check yet.
        </p>

        {notes.length > 0 ? (
          <section>
            <h4 style={sectionHeadingStyle}>Notes</h4>
            <ul style={listStyle}>
              {notes.map((n) => (
                <ReviewRow
                  key={n.id}
                  identity={n.text}
                  disclosure={DISCLOSURE.note}
                  receiverLabel={RECEIVER_LABEL.note}
                />
              ))}
            </ul>
          </section>
        ) : null}

        {sessions.length > 0 ? (
          <section>
            <h4 style={sectionHeadingStyle}>Sessions</h4>
            <ul style={listStyle}>
              {sessions.map((s) => (
                <ReviewRow
                  key={s.id}
                  identity={s.title}
                  disclosure={DISCLOSURE.session}
                  receiverLabel={RECEIVER_LABEL.session}
                />
              ))}
            </ul>
          </section>
        ) : null}

        {ingests.length > 0 ? (
          <section>
            <h4 style={sectionHeadingStyle}>Files</h4>
            <ul style={listStyle}>
              {ingests.map((f) => (
                <ReviewRow
                  key={f.id}
                  identity={f.path}
                  disclosure={DISCLOSURE.ingest}
                  receiverLabel={RECEIVER_LABEL.ingest}
                />
              ))}
            </ul>
          </section>
        ) : null}

        <label style={{ fontSize: 12, color: "var(--text-secondary)", display: "grid", gap: 4 }}>
          A note for whoever receives this (optional)
          <input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What is this for?"
            style={{ padding: "8px 12px", borderRadius: 6, fontFamily: "inherit", fontSize: 14 }}
          />
        </label>

        {error ? (
          <p style={{ fontSize: 13, color: "var(--error)", margin: 0, lineHeight: 1.5 }}>{error}</p>
        ) : null}

        <div style={{ display: "flex", gap: 8 }}>
          <Button variant="primary" disabled={exporting || total === 0} onClick={() => onConfirm(description)}>
            {exporting ? "Sealing…" : "Export signed bundle"}
          </Button>
          <Button variant="secondary" disabled={exporting} onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}
