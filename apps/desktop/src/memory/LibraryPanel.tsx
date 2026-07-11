import { useEffect, useRef, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  listSessions, listNotes, recall, getSession, deleteSession,
  type SessionSummaryDto, type SessionDetailDto, type NoteDto, type HitDto,
} from "../api/engine";
import { HitList } from "./HitList";
import { SessionReader } from "./SessionReader";
import { formatDay } from "./format";

/** The daemon's bare-string rejection when a session no longer exists (delete race — spec §3). */
const SESSION_GONE = "session not found or deleted";
/** Tauri rejects Result<_, String> with a BARE STRING; match the gone-race arm on its text. */
const isGone = (e: unknown) => String(e).includes(SESSION_GONE);

/** How many hits to request when the user runs a full-memory search. */
const RECALL_K = 10;

/** Case-insensitive "contains" — the primitive behind the client-side filter. */
function matches(haystack: string, needle: string): boolean {
  return haystack.toLowerCase().includes(needle);
}

/**
 * The Library: browse and search everything your agent has remembered — captured sessions and
 * the notes it has kept. One search box does double duty: typing filters the already-loaded lists
 * instantly (client-side, over title/project/note text), while "Search memory" runs a full recall
 * across the whole brain and shows the hits in a separate Memory group (reusing the Brain search's
 * row rendering). Session View/Delete (C5) and note Supersede + the recall-stats strip (C6) attach
 * at the seams marked below. Tokens only (no hardcoded colors) per the shell-redesign gate.
 */
export function LibraryPanel() {
  const [sessions, setSessions] = useState<SessionSummaryDto[]>([]);
  const [notes, setNotes] = useState<NoteDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // One search box: `query` drives the instant client-side filter AND is the recall query.
  const [query, setQuery] = useState("");

  // Full-brain recall (the daemon search), kept separate from the client-side filter.
  const [hits, setHits] = useState<HitDto[]>([]);
  const [searched, setSearched] = useState(false);
  // The term the shown hits are FOR — the live filter keeps re-filtering as you type, so the recall
  // group labels itself with the searched term to make clear its hits aren't the live filter.
  const [searchedTerm, setSearchedTerm] = useState("");
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const [s, n] = await Promise.all([listSessions(), listNotes()]);
      setSessions(s);
      setNotes(n);
    } catch (e) {
      // Tauri rejects Result<_, String> with a BARE STRING — surface it as-is, never read `.message`.
      setLoadError(String(e));
    } finally {
      setLoading(false);
    }
  };
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => { void loadRef.current(); }, []);

  const onSearchMemory = async () => {
    const q = query.trim();
    if (!q) return;
    setSearching(true);
    setSearchError(null);
    try {
      setHits(await recall(q, RECALL_K));
      setSearchedTerm(q);
      setSearched(true);
    } catch (e) {
      setSearchError(String(e));
    } finally {
      setSearching(false);
    }
  };

  // ---- C5: session reader (a slide-over showing one session's rendered Markdown) ----
  const [openSessionId, setOpenSessionId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetailDto | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [detailGone, setDetailGone] = useState(false);

  // ---- C5: honest Delete flow (confirm gate + already-deleted notice) ----
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const closeReader = () => {
    setOpenSessionId(null);
    setDetail(null);
    setDetailError(null);
    setDetailGone(false);
  };

  const openReader = async (sessionId: string) => {
    setNotice(null);
    setOpenSessionId(sessionId);
    setDetail(null);
    setDetailError(null);
    setDetailGone(false);
    setDetailLoading(true);
    try {
      setDetail(await getSession(sessionId));
    } catch (e) {
      if (isGone(e)) {
        // Deleted elsewhere between listing and opening → friendly reader state + authoritative refresh.
        setDetailGone(true);
        void loadRef.current();
      } else {
        setDetailError(String(e));
      }
    } finally {
      setDetailLoading(false);
    }
  };

  const askDelete = (sessionId: string) => {
    setNotice(null);
    setDeleteError(null);
    setConfirmDeleteId(sessionId);
  };

  const confirmDelete = async () => {
    const sessionId = confirmDeleteId;
    if (!sessionId) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteSession(sessionId);
      // Drop the row locally; close the reader if it was showing this session.
      setSessions((prev) => prev.filter((s) => s.session_id !== sessionId));
      if (openSessionId === sessionId) closeReader();
      setConfirmDeleteId(null);
    } catch (e) {
      if (isGone(e)) {
        // deleteSession is idempotent, but the race arm is honest either way: friendly notice + refresh.
        setNotice("That session was already deleted.");
        if (openSessionId === sessionId) closeReader();
        setConfirmDeleteId(null);
        void loadRef.current();
      } else {
        setDeleteError(String(e));
      }
    } finally {
      setDeleting(false);
    }
  };

  if (loading) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Library</h2>
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>Loading your library…</p>
      </Card>
    );
  }

  if (loadError) {
    return (
      <Card>
        <h2 style={{ margin: "0 0 8px" }}>Library</h2>
        <p style={{ fontSize: 13, color: "var(--error)", margin: "0 0 12px" }}>
          Couldn’t load your library: {loadError}
        </p>
        <Button variant="secondary" onClick={() => void load()}>Try again</Button>
      </Card>
    );
  }

  // Newest-first, then case-insensitive contains over the fields each list exposes.
  const needle = query.trim().toLowerCase();
  const visibleSessions = [...sessions]
    .sort((a, b) => b.started_at - a.started_at)
    .filter((s) => needle === "" || matches(s.title, needle) || matches(s.project, needle));
  const visibleNotes = [...notes]
    .sort((a, b) => b.created_at - a.created_at)
    .filter((n) => needle === "" || matches(n.text, needle));
  const archiveEmpty = sessions.length === 0 && notes.length === 0;

  return (
    <Card>
      <h2 style={{ margin: "0 0 4px" }}>Library</h2>
      <p style={{ color: "var(--text-secondary)", fontSize: 13, margin: "0 0 12px", lineHeight: 1.5 }}>
        Everything your agent remembers, in one place — filter what’s loaded, or search your whole memory.
      </p>

      {notice ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)", margin: "0 0 12px" }}>{notice}</p>
      ) : null}

      <div style={{ display: "flex", gap: 8, margin: "12px 0 4px" }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") void onSearchMemory(); }}
          placeholder="Filter sessions & notes…"
          aria-label="Filter your library, or press Enter to search all memory"
          style={{ flex: 1, padding: "8px 12px", borderRadius: 6, fontFamily: "inherit", fontSize: 14 }}
        />
        <Button variant="primary" onClick={onSearchMemory} disabled={searching || query.trim() === ""}>
          {searching ? "Searching…" : "Search memory"}
        </Button>
      </div>
      <p style={{ color: "var(--text-tertiary)", fontSize: 12, margin: "0 0 12px" }}>
        Typing filters what’s loaded; press Enter or “Search memory” to search your whole memory.
      </p>

      {searchError ? <p style={{ fontSize: 13, color: "var(--error)" }}>{searchError}</p> : null}

      {/* Full-brain recall results — a SEPARATE group from the client-side-filtered lists below. */}
      {searched && !searchError ? (
        <section style={{ marginBottom: 20 }}>
          <h3 style={{ fontSize: 13, margin: "0 0 8px" }}>Memory · “{searchedTerm}”</h3>
          {hits.length === 0 ? (
            <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>Nothing found in memory — try different words.</p>
          ) : (
            <HitList hits={hits} />
          )}
        </section>
      ) : null}

      {archiveEmpty ? (
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
          Your captured sessions and notes will appear here.
        </p>
      ) : (
        <>
          <section style={{ marginBottom: 20 }}>
            <h3 style={{ fontSize: 13, margin: "0 0 8px" }}>Sessions</h3>
            {visibleSessions.length === 0 ? (
              <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
                {needle === "" ? "No sessions yet." : `No sessions match “${query.trim()}”.`}
              </p>
            ) : (
              <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
                {visibleSessions.map((s) => (
                  <li key={s.session_id} style={{ padding: "10px 0", borderBottom: "1px solid var(--border-soft)" }}>
                    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
                      <div>
                        <div style={{ fontSize: 14, fontWeight: 600 }}>{s.title}</div>
                        <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                          <span>{s.project}</span> · <span>{formatDay(s.started_at)}</span>
                        </div>
                      </div>
                      {/* C5: session View (opens the reader) / Delete (honest confirm) row actions. */}
                      <div style={{ display: "flex", gap: 8, flexShrink: 0 }}>
                        <Button variant="secondary" onClick={() => void openReader(s.session_id)}>View</Button>
                        <Button variant="secondary" className="danger-btn" onClick={() => askDelete(s.session_id)}>Delete</Button>
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section>
            <h3 style={{ fontSize: 13, margin: "0 0 8px" }}>Notes</h3>
            {visibleNotes.length === 0 ? (
              <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
                {needle === "" ? "No notes yet." : `No notes match “${query.trim()}”.`}
              </p>
            ) : (
              <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
                {visibleNotes.map((n) => (
                  <li key={n.event_id} style={{ padding: "10px 0", borderBottom: "1px solid var(--border-soft)" }}>
                    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
                      <div>
                        <div style={{ fontSize: 14, lineHeight: 1.4 }}>{n.text}</div>
                        <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>{formatDay(n.created_at)}</div>
                      </div>
                      {/* C6 seam: per-note Supersede row action attaches here. */}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}

      <SessionReader
        isOpen={openSessionId !== null}
        detail={detail}
        loading={detailLoading}
        error={detailError}
        gone={detailGone}
        onClose={closeReader}
        onDelete={() => { if (openSessionId) askDelete(openSessionId); }}
      />

      {/* Honest Delete confirm — an overlay (z above the reader) quoting §7b: the readable body is
          destroyed, but the session's title stays in the append-only signed chain (not erasable yet). */}
      {confirmDeleteId !== null ? (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={() => { if (!deleting) setConfirmDeleteId(null); }}
        >
          <div
            className="modal-card"
            role="dialog"
            aria-modal="true"
            aria-label="Delete session"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <h3 style={{ color: "var(--error)" }}>Delete this session?</h3>
            <p style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.5, margin: 0 }}>
              Its saved text — the readable note stored on your Mac — will be permanently deleted.
              The session’s title remains in the encrypted log: the signed history chain can’t be
              erased yet, so the title can’t be fully forgotten today.
            </p>
            {deleteError ? (
              <p style={{ fontSize: 13, color: "var(--error)", margin: 0 }}>{deleteError}</p>
            ) : null}
            <div style={{ display: "flex", gap: 8 }}>
              <Button variant="secondary" className="danger-btn" disabled={deleting} onClick={() => void confirmDelete()}>
                {deleting ? "Deleting…" : "Delete session"}
              </Button>
              <Button variant="secondary" disabled={deleting} onClick={() => setConfirmDeleteId(null)}>
                Cancel
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </Card>
  );
}
