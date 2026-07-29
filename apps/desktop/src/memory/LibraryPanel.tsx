import { useEffect, useRef, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  listSessions, listNotes, listFiles, recall, getSession, deleteSession, recallStats,
  type SessionSummaryDto, type SessionDetailDto, type NoteDto, type FileRecordDto,
  type HitDto, type RecallStatsDto,
} from "../api/engine";
import { exportBundle } from "../api/export";
import { HitList } from "./HitList";
import { SessionReader } from "./SessionReader";
import { NoteRow } from "./NoteRow";
import { ExportReviewSheet } from "./ExportReviewSheet";
import { formatDay } from "./format";

/** The daemon's bare-string rejection when a session no longer exists (delete race — spec §3). */
const SESSION_GONE = "session not found or deleted";
/** Tauri rejects Result<_, String> with a BARE STRING; match the gone-race arm on its text. */
const isGone = (e: unknown) => String(e).includes(SESSION_GONE);

/** How many hits to request when the user runs a full-memory search. */
const RECALL_K = 10;

/** Add or remove `id` from a selection set, returning a NEW set (React state must not mutate). */
function toggled(set: Set<string>, id: string): Set<string> {
  const next = new Set(set);
  if (!next.delete(id)) next.add(id);
  return next;
}

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
 * at the seams marked below, and Rung-5 export (tick memories → review sheet → one signed `.airmem`)
 * at the seam after them. Tokens only (no hardcoded colors) per the shell-redesign gate.
 */
export function LibraryPanel() {
  const [sessions, setSessions] = useState<SessionSummaryDto[]>([]);
  const [notes, setNotes] = useState<NoteDto[]>([]);
  const [files, setFiles] = useState<FileRecordDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // C6: recall telemetry (queries-only miss log — spec §8). Since Rung-4 R4-A these miss queries also
  // ACTIVELY drive the reflect loop's dossier refreshes (design §2.4) — no longer a passive display; a
  // stats hiccup must never break the Library, so it loads on its own and degrades to a hidden strip on
  // failure.
  const [stats, setStats] = useState<RecallStatsDto | null>(null);

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

  // Re-list notes after a supersede: the daemon returns the new current note and drops the old one.
  const refreshNotes = async () => {
    try {
      setNotes(await listNotes());
    } catch {
      // The supersede already SAVED — only the re-list failed. Keep the whole panel up with a
      // localized, non-fatal notice (never the fatal loadError card that would blank the Library);
      // the honest message is "saved, couldn't refresh," not "couldn't load."
      setNotice("Saved — reload to see the update.");
    }
  };

  const loadStats = async () => {
    // Degrade quietly — a failed stats read hides the strip, it never breaks the Library.
    try {
      setStats(await recallStats());
    } catch {
      setStats(null);
    }
  };
  const loadStatsRef = useRef(loadStats);
  loadStatsRef.current = loadStats;

  const loadFiles = async () => {
    // Its OWN loader, not part of the sessions+notes `Promise.all`: an ingest-list hiccup must hide
    // the Files section, never blank the whole Library (same rule as the stats strip).
    try {
      setFiles(await listFiles());
    } catch {
      setFiles([]);
    }
  };
  const loadFilesRef = useRef(loadFiles);
  loadFilesRef.current = loadFiles;

  useEffect(() => {
    void loadRef.current();
    void loadStatsRef.current();
    void loadFilesRef.current();
  }, []);

  const onSearchMemory = async () => {
    const q = query.trim();
    if (!q) return;
    setSearching(true);
    setSearchError(null);
    try {
      setHits(await recall(q, RECALL_K));
      setSearchedTerm(q);
      setSearched(true);
      // An in-panel recall may have recorded a miss — refresh the strip so it reflects it now,
      // not only after a remount. Best-effort and quiet-on-failure, like the mount load.
      void loadStats();
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

  // ---- Rung-5 SP-V1: export a selection as a signed .airmem ----
  //
  // All three classes. A session is addressed by its CAPTURE EVENT id (`capture_event_id`), NEVER
  // by `session_id` — the two are different values, and the daemon refuses the wrong one with
  // "… is not a current session". The selection sets therefore hold capture event ids, which is
  // also what `deleteSession`'s `session_id` calls elsewhere in this panel must not be confused with.
  const [selectedSessions, setSelectedSessions] = useState<Set<string>>(new Set());
  const [selectedNotes, setSelectedNotes] = useState<Set<string>>(new Set());
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [reviewing, setReviewing] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);

  // Resolve the selection against the CURRENT lists, so anything that stopped being current between
  // selecting and exporting silently drops out instead of being sent as a stale id the daemon would
  // refuse — a note that was superseded, a file that was re-ingested, or a session that was deleted
  // or re-captured (a new capture supersedes the old event, so the fold head's id changes).
  const chosenSessions = sessions.filter((s) => selectedSessions.has(s.capture_event_id));
  const chosenNotes = notes.filter((n) => selectedNotes.has(n.event_id));
  const chosenFiles = files.filter((f) => selectedFiles.has(f.file_event_id));
  const chosenCount = chosenSessions.length + chosenNotes.length + chosenFiles.length;

  const openReview = () => {
    setNotice(null);
    setExportError(null);
    setReviewing(true);
  };

  const confirmExport = async (description: string) => {
    setExporting(true);
    setExportError(null);
    try {
      const path = await exportBundle(
        chosenNotes.map((n) => n.event_id),
        // The CAPTURE EVENT id, not `s.session_id` — the daemon refuses the latter.
        chosenSessions.map((s) => s.capture_event_id),
        chosenFiles.map((f) => f.file_event_id),
        description,
      );
      setReviewing(false);
      // A cancelled save dialog yields null — nothing was written, so say nothing happened.
      setNotice(path === null ? "Export cancelled — nothing was saved." : `Saved to ${path}`);
      if (path !== null) {
        setSelectedSessions(new Set());
        setSelectedNotes(new Set());
        setSelectedFiles(new Set());
      }
    } catch (e) {
      // Tauri rejects Result<_, String> with a BARE STRING. Show the daemon's refusal verbatim —
      // which memory it refused, and why, is the whole point.
      setExportError(String(e));
    } finally {
      setExporting(false);
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
        {/* Retry ALL three lists: `load` covers sessions+notes, `loadFiles` is its own quiet loader,
            and a retry that silently skipped one would leave the Files section empty for the rest
            of the session with no way to ask again. */}
        <Button variant="secondary" onClick={() => { void load(); void loadFiles(); }}>Try again</Button>
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
  const visibleFiles = files.filter((f) => needle === "" || matches(f.canonical_path, needle));
  const archiveEmpty = sessions.length === 0 && notes.length === 0 && files.length === 0;

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
          placeholder="Filter sessions, notes & files…"
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

      {/* Rung-5 SP-V1: turn a selection into one signed .airmem the receiver can verify. */}
      {archiveEmpty ? null : (
        <div style={{ display: "flex", alignItems: "center", gap: 8, margin: "0 0 16px", flexWrap: "wrap" }}>
          <Button variant="secondary" disabled={chosenCount === 0} onClick={openReview}>
            {chosenCount === 0 ? "Export signed bundle" : `Export signed bundle (${chosenCount})`}
          </Button>
          <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
            Tick sessions, notes, or files to send them as one signed file.
          </span>
        </div>
      )}

      {/* C6: recall-miss stats strip. Since R4-A these queries actively drive reflection (dossier
          refreshes for missed topics — design §2.4). Still only past search QUERIES that found nothing
          are shown (never result text — spec §8; the daemon guarantees queries-only). */}
      {stats && stats.total > 0 ? (
        <div style={{ margin: "0 0 16px", fontSize: 12, color: "var(--text-tertiary)", lineHeight: 1.5 }}>
          <div>
            <span>{stats.total} recalls</span>
            {" · "}
            <span>{stats.misses} found nothing</span>
          </div>
          {stats.recent_misses.length > 0 ? (
            <div style={{ marginTop: 4 }}>
              <span>Recent searches that found nothing:</span>
              <ul style={{ listStyle: "none", padding: 0, margin: "4px 0 0", display: "flex", flexWrap: "wrap", gap: 6 }}>
                {stats.recent_misses.slice(0, 5).map((m, i) => (
                  <li
                    // Query+time can repeat if the same term misses twice within a second; the
                    // index makes the key unique (avoids React's duplicate-key warning).
                    key={`${m.query} ${m.at} ${i}`}
                    style={{ padding: "2px 8px", borderRadius: 6, border: "1px solid var(--border-soft)" }}
                  >
                    <span>“{m.query}”</span>
                    <span style={{ marginLeft: 6 }}>{formatDay(m.at)}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      ) : null}

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
          Your captured sessions, notes, and ingested files will appear here.
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
                      <div style={{ display: "flex", alignItems: "flex-start", gap: 8 }}>
                        <input
                          type="checkbox"
                          // Keyed by the CAPTURE EVENT id — the export address, not `session_id`.
                          checked={selectedSessions.has(s.capture_event_id)}
                          onChange={() =>
                            setSelectedSessions((prev) => toggled(prev, s.capture_event_id))
                          }
                          aria-label={`Select session for export: ${s.title}`}
                          style={{ marginTop: 3, flexShrink: 0 }}
                        />
                        <div>
                          <div style={{ fontSize: 14, fontWeight: 600 }}>{s.title}</div>
                          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                            <span>{s.project}</span> · <span>{formatDay(s.started_at)}</span>
                          </div>
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
                {/* C6: each note is editable in place (Supersede). A saved edit re-lists notes so the
                    replacement shows and the old text vanishes (the daemon returns current notes only). */}
                {visibleNotes.map((n) => (
                  <NoteRow
                    key={n.event_id}
                    note={n}
                    selected={selectedNotes.has(n.event_id)}
                    onToggleSelected={() => setSelectedNotes((prev) => toggled(prev, n.event_id))}
                    onSaved={refreshNotes}
                  />
                ))}
              </ul>
            )}
          </section>

          {files.length > 0 ? (
            <section style={{ marginTop: 20 }}>
              <h3 style={{ fontSize: 13, margin: "0 0 8px" }}>Files</h3>
              {visibleFiles.length === 0 ? (
                <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
                  {`No files match “${query.trim()}”.`}
                </p>
              ) : (
                <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
                  {visibleFiles.map((f) => (
                    <li
                      key={f.file_event_id}
                      style={{ padding: "10px 0", borderBottom: "1px solid var(--border-soft)" }}
                    >
                      <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                        <input
                          type="checkbox"
                          checked={selectedFiles.has(f.file_event_id)}
                          onChange={() =>
                            setSelectedFiles((prev) => toggled(prev, f.file_event_id))
                          }
                          aria-label={`Select file for export: ${f.canonical_path}`}
                          style={{ flexShrink: 0 }}
                        />
                        <span style={{ wordBreak: "break-word" }}>{f.canonical_path}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : null}
        </>
      )}

      {reviewing ? (
        <ExportReviewSheet
          notes={chosenNotes.map((n) => ({ id: n.event_id, text: n.text }))}
          sessions={chosenSessions.map((s) => ({ id: s.capture_event_id, title: s.title }))}
          ingests={chosenFiles.map((f) => ({ id: f.file_event_id, path: f.canonical_path }))}
          exporting={exporting}
          error={exportError}
          onConfirm={(description) => void confirmExport(description)}
          onCancel={() => setReviewing(false)}
        />
      ) : null}

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
