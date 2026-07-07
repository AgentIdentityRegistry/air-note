import { useEffect, useRef, useState } from "react";
import { SettingsSectionCard } from "../components/ui/SettingsSectionCard";
import { Button } from "../components/Button";
import { downloadLanguagePack, modelStatus, type ModelStatusDto } from "../api/engine";

/** How often the card refreshes the language-pack state while the tab is open. */
const POLL_MS = 2000;
/** Approximate on-disk size of the multilingual pack, surfaced before the user commits to it. */
const PACK_SIZE_LABEL = "~500 MB";

type Props = {
  /** Whether the multilingual model is already on disk (drives the Enable vs. active copy). */
  installed: boolean;
};

/**
 * Brain → Search & Evolve card that turns on multilingual (e.g. Korean) memory search: it downloads
 * a language pack, then the daemon re-indexes in the background while English search keeps working.
 * Polls `modelStatus` and fails soft — a transport blip while the daemon restarts leaves the
 * last-known state on screen instead of crashing the panel.
 */
export function LanguagePackCard({ installed }: Props) {
  const [status, setStatus] = useState<ModelStatusDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus(await modelStatus());
    } catch {
      // Payload-encoded on the happy path; a transport blip just keeps the last state.
    }
  };
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    void refreshRef.current();
    const id = setInterval(() => void refreshRef.current(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onEnable = async () => {
    setBusy(true);
    setError(null);
    try {
      await downloadLanguagePack();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const failed = status?.state === "failed";
  const needsRedownload = status?.state === "missing" || status?.state === "mismatch";
  // Narrow once so the progress copy needs no non-null assertions.
  const reindex =
    status && status.reindex_done != null && status.reindex_total != null && status.reindex_total > 0
      ? { done: status.reindex_done, total: status.reindex_total }
      : null;

  return (
    <SettingsSectionCard
      title="Multilingual search"
      description="Search your memory in Korean and other languages. Adds a downloadable model; English search stays available the whole time."
    >
      {failed ? (
        <div>
          <p style={{ fontSize: 13, color: "var(--error)" }}>
            Multilingual couldn’t turn on: {status?.reason ?? "the re-index failed"}. English search
            is unaffected — retry when you’re ready.
          </p>
          <Button variant="primary" onClick={onEnable} disabled={busy}>
            {busy ? "Working…" : "Retry"}
          </Button>
        </div>
      ) : needsRedownload ? (
        <div>
          <p style={{ fontSize: 13, color: "var(--error)" }}>
            {status?.state === "missing"
              ? "Model files are missing — re-download to restore multilingual search."
              : "Model files failed their integrity check — re-download to restore multilingual search."}
          </p>
          <Button variant="primary" onClick={onEnable} disabled={busy}>
            {busy ? "Working…" : "Re-download"}
          </Button>
        </div>
      ) : reindex ? (
        <p style={{ fontSize: 13 }}>
          Re-indexing your memories {reindex.done.toLocaleString()} /{" "}
          {reindex.total.toLocaleString()} — English search stays available; multilingual turns on
          when this finishes.
        </p>
      ) : installed ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>Multilingual active.</p>
      ) : (
        <div>
          <Button variant="primary" onClick={onEnable} disabled={busy}>
            {busy ? "Starting…" : `Enable multilingual (${PACK_SIZE_LABEL})`}
          </Button>
          {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
        </div>
      )}
    </SettingsSectionCard>
  );
}
