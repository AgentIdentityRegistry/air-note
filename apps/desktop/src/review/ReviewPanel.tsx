import { useEffect, useRef, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  listProposals, proposalPreview, applyProposal, declineProposal, undoApply,
  type ProposalDto, type PreviewDto,
} from "../api/engine";
import { toProposalRow } from "./proposalView";
import { inlineDiff } from "./diffView";
import { runApprove } from "./applyFlow";

/** How often the queue refreshes while the Review tab is open. */
const POLL_MS = 5000;

type Applied = { fileWrittenId: string; fileName: string };

export function ReviewPanel() {
  const [proposals, setProposals] = useState<ProposalDto[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewDto | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmFor, setConfirmFor] = useState<string | null>(null);
  const [reviewed, setReviewed] = useState(false);
  const [applied, setApplied] = useState<Applied[]>([]);
  // Synchronous in-flight guard: `busy` state only updates after a re-render, so a fast
  // double-click would fire two applies before `disabled` takes effect. This ref flips
  // synchronously, so the second click is a true no-op (no stale-banner flash).
  const inFlight = useRef(false);

  const refresh = async () => {
    try {
      setProposals(await listProposals());
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onOpen = async (id: string) => {
    setOpenId(id);
    setPreview(null);
    setError(null);
    setPreviewing(true);
    try {
      setPreview(await proposalPreview(id));
    } catch (e) {
      setError(String(e));
    } finally {
      setPreviewing(false);
    }
  };

  // The engine op is authoritative for the loud-confirm (item 2 / G1): `runApprove` calls
  // applyProposal(id, acknowledged) and classifies the typed errors via the unit-tested
  // `classifyApplyError` (the SINGLE source of the loud/stale string match). When the FRESH
  // re-gate is loud and we passed false, the outcome is `needsLoud` → open the modal and retry
  // with true. A `stale` outcome means the file changed under us → clear + reload the preview
  // (MIN-2) so the next render shows the new baseline, not the stale diff.
  const doApply = async (id: string, fileName: string, acknowledged: boolean) => {
    if (inFlight.current) return; // synchronous double-apply guard (see `inFlight`).
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      const outcome = await runApprove(applyProposal, id, acknowledged);
      if ("applied" in outcome) {
        setApplied((prev) => [{ fileWrittenId: outcome.applied, fileName }, ...prev]);
        setOpenId(null);
        setPreview(null);
        setConfirmFor(null);
        setReviewed(false);
        await refresh();
      } else if ("needsLoud" in outcome) {
        // Authoritative loud gate fired — surface the "I've reviewed this" modal, no error text.
        setConfirmFor(id);
        setReviewed(false);
      } else if ("stale" in outcome) {
        setError("The file changed since this was suggested — reloading the diff.");
        setConfirmFor(null);
        setReviewed(false);
        if (openId === id) { setPreview(null); void onOpen(id); } // MIN-2: reload the baseline.
        await refresh();
      } else {
        setError(outcome.error);
      }
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  // Pre-show the modal if the propose-time HINT is loud; otherwise try the op (which still
  // re-checks loud on the fresh verdict and will bounce back NeedsLoudConfirm if needed).
  const onApprove = (id: string, requiresLoudHint: boolean, fileName: string) => {
    if (inFlight.current) return; // ignore a fast second click before the first apply resolves.
    if (requiresLoudHint) {
      setConfirmFor(id);
      setReviewed(false);
    } else {
      void doApply(id, fileName, false);
    }
  };

  const onDecline = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await declineProposal(id, "declined in Review");
      setOpenId(null);
      setPreview(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onUndo = async (fileWrittenId: string) => {
    setBusy(true);
    setError(null);
    try {
      await undoApply(fileWrittenId);
      setApplied((prev) => prev.filter((a) => a.fileWrittenId !== fileWrittenId));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Review</h2>
        <p style={{ color: "#666" }}>Couldn’t reach the memory engine. Set up your identity first, then enable a folder for edits.</p>
      </Card>
    );
  }

  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Review</h2>
      {error ? <p style={{ color: "#b00", fontSize: 13 }}>{error}</p> : null}

      {proposals.length === 0 ? (
        <Card>
          <p style={{ color: "#666" }}>
            No changes to review. When the brain learns something that contradicts a file in an
            edit-enabled folder (and evolve is on), proposed rewrites appear here.
          </p>
          <p style={{ color: "#888", fontSize: 12 }}>
            Note: only contradictions learned <em>after</em> you enable a folder for edits are
            proposed — corrections learned earlier won’t be re-applied retroactively.
          </p>
        </Card>
      ) : (
        proposals.map((p) => {
          const row = toProposalRow(p);
          const isOpen = openId === p.id;
          return (
            <Card key={p.id}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
                <div>
                  <div style={{ fontWeight: 600 }}>
                    {row.opLabel}: <code>{row.fileName}</code>{" "}
                    {row.fromMandate ? <span style={{ color: "#06c", fontSize: 12 }}>· from a mandate</span> : null}{" "}
                    {row.risky ? <span style={{ color: "#b00", fontSize: 12 }}>⚠ needs careful review</span> : null}
                  </div>
                  <div style={{ color: "#666", fontSize: 12 }}><code>{row.folder}</code> · enabled ✓</div>
                  <div style={{ fontSize: 13, marginTop: 4 }}>Why: {row.why}</div>
                </div>
                <Button variant="secondary" onClick={() => (isOpen ? setOpenId(null) : void onOpen(p.id))}>
                  {isOpen ? "Hide" : "Preview"}
                </Button>
              </div>

              {isOpen ? (
                <div style={{ marginTop: 8 }}>
                  {previewing ? (
                    <p style={{ color: "#666", fontSize: 13 }}>Loading preview…</p>
                  ) : preview ? (
                    <>
                      <pre style={{ background: "#f6f6f6", padding: 8, fontSize: 12, overflowX: "auto", margin: 0 }}>
                        {inlineDiff(preview.old_text, preview.new_text).map((line, idx) => (
                          <div
                            key={idx}
                            style={{
                              color: line.kind === "del" ? "#b00" : line.kind === "add" ? "#070" : "#444",
                              background: line.kind === "del" ? "#fdecea" : line.kind === "add" ? "#eafaef" : "transparent",
                            }}
                          >
                            {line.kind === "del" ? "- " : line.kind === "add" ? "+ " : "  "}
                            {line.text}
                          </div>
                        ))}
                      </pre>
                      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                        <Button variant="primary" disabled={busy} onClick={() => onApprove(p.id, preview.requires_loud_modal, row.fileName)}>
                          Approve
                        </Button>
                        <Button variant="secondary" disabled={busy} onClick={() => void onDecline(p.id)}>
                          Decline
                        </Button>
                      </div>
                    </>
                  ) : null}
                </div>
              ) : null}
            </Card>
          );
        })
      )}

      {confirmFor ? (
        <Card>
          <div style={{ fontWeight: 600, color: "#b00" }}>Confirm this edit</div>
          <p style={{ fontSize: 13, color: "#444" }}>
            This rewrites a file your agent learned from. Review the before/after, then confirm you’ve read it.
          </p>
          <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13 }}>
            <input type="checkbox" checked={reviewed} onChange={(e) => setReviewed(e.target.checked)} />
            I’ve reviewed this
          </label>
          <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
            <Button
              variant="primary"
              disabled={!reviewed || busy}
              onClick={() => {
                const target = proposals.find((p) => p.id === confirmFor);
                if (target) {
                  void doApply(confirmFor, toProposalRow(target).fileName, true);
                } else {
                  // A poll removed this proposal while the modal was open → no dead button.
                  setConfirmFor(null);
                  setReviewed(false);
                  setError("This proposal is no longer available.");
                }
              }}
            >
              Apply anyway
            </Button>
            <Button variant="secondary" disabled={busy} onClick={() => { setConfirmFor(null); setReviewed(false); }}>
              Cancel
            </Button>
          </div>
        </Card>
      ) : null}

      {applied.length > 0 ? (
        <Card>
          <div style={{ fontWeight: 600 }}>Recently applied</div>
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {applied.map((a) => (
              <li key={a.fileWrittenId} style={{ marginBottom: 4 }}>
                <code>{a.fileName}</code>{" "}
                <button onClick={() => void onUndo(a.fileWrittenId)} disabled={busy} style={{ marginLeft: 8 }}>Undo</button>
              </li>
            ))}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}
