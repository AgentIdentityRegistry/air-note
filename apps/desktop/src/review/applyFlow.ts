import type { ApplyResultDto } from "../api/engine";

/** Classify an engine apply error by its message text (the typed errors stringify distinctly). */
export type ApplyErrorKind = "loud" | "stale" | "other";
export function classifyApplyError(message: string): ApplyErrorKind {
  if (message.includes("needs an explicit review confirmation")) return "loud";
  if (message.includes("changed since this was suggested")) return "stale";
  return "other";
}

/** The outcome of one approve attempt. */
export type ApproveOutcome =
  | { applied: string }   // file_written id
  | { needsLoud: true }   // the op refused — caller must show the modal + retry with ack=true
  | { stale: true }       // the file changed — caller reloads the preview
  | { error: string };

/** Run an approve through the apply op. `op(id, acknowledged)` is `applyProposal`. The op is the
 *  authoritative loud gate: without the ack a loud proposal throws NeedsLoudConfirm → `needsLoud`. */
export async function runApprove(
  op: (id: string, acknowledged: boolean) => Promise<ApplyResultDto>,
  id: string,
  acknowledged: boolean,
): Promise<ApproveOutcome> {
  try {
    const r = await op(id, acknowledged);
    return { applied: r.file_written_id };
  } catch (e) {
    const msg = String(e instanceof Error ? e.message : e);
    const kind = classifyApplyError(msg);
    if (kind === "loud" && !acknowledged) return { needsLoud: true };
    if (kind === "stale") return { stale: true };
    return { error: msg };
  }
}
