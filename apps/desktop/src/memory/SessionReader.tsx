import { SlidePanel } from "../components/ui/SlidePanel";
import { Button } from "../components/Button";
import { formatDay } from "./format";
import type { SessionDetailDto } from "../api/engine";

/**
 * The session reader: a slide-over panel showing one captured session's summary and its
 * daemon-rendered Markdown body, plus a Delete affordance. The Markdown is EXTERNAL-TAINTED captured
 * content, so it renders as PREFORMATTED TEXT (a <pre>) and NEVER via dangerouslySetInnerHTML — the
 * desktop UI must not execute captured content as markup. Loading / already-deleted / error states
 * are driven by the parent (which owns the getSession call and the delete-race handling).
 */
export function SessionReader({
  isOpen, detail, loading, error, gone, onClose, onDelete,
}: {
  isOpen: boolean;
  detail: SessionDetailDto | null;
  loading: boolean;
  error: string | null;
  gone: boolean;
  onClose: () => void;
  onDelete: () => void;
}) {
  return (
    <SlidePanel isOpen={isOpen} onClose={onClose} title="Session">
      <div style={{ padding: 20, display: "flex", flexDirection: "column", height: "100%", gap: 12 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: 8 }}>
          <h2 style={{ margin: 0, fontSize: 18 }}>{detail?.summary.title ?? "Session"}</h2>
          <Button variant="secondary" onClick={onClose}>Close</Button>
        </div>

        {detail ? (
          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
            {detail.summary.project} · {formatDay(detail.summary.started_at)}
          </div>
        ) : null}

        {loading ? (
          <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>Loading session…</p>
        ) : gone ? (
          <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>This session was already deleted.</p>
        ) : error ? (
          <p style={{ color: "var(--error)", fontSize: 13 }}>Couldn’t open this session: {error}</p>
        ) : detail ? (
          <>
            {/* External-tainted captured content → render as text, not HTML. */}
            <pre
              style={{
                background: "var(--surface-soft)", padding: 12, borderRadius: 8, fontSize: 13,
                whiteSpace: "pre-wrap", wordBreak: "break-word", overflowY: "auto", margin: 0,
                flex: 1, fontFamily: "inherit", lineHeight: 1.5,
              }}
            >
              {detail.markdown}
            </pre>
            <div>
              <Button variant="secondary" className="danger-btn" onClick={onDelete}>Delete</Button>
            </div>
          </>
        ) : null}
      </div>
    </SlidePanel>
  );
}
