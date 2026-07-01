import type { ReasonerConfigDto } from "../api/engine";
import { bannerText, cloudActive, taintedNotice } from "./reasonerView";

/** Persistent, non-dismissible indicator shown only while cloud reasoning is active.
 *  `taintedSnippets` is the last cloud tick's file-derived egress count (null until one runs). */
export function CloudEgressBanner({
  cfg,
  taintedSnippets = null,
}: {
  cfg: ReasonerConfigDto | null;
  taintedSnippets?: number | null;
}) {
  if (!cfg || !cloudActive(cfg)) return null;
  const notice = taintedNotice(taintedSnippets);
  return (
    <div
      role="status"
      style={{
        marginTop: 12, padding: "8px 12px", borderRadius: 6,
        border: "1px solid var(--error)", color: "var(--error)",
        background: "var(--surface-soft)", fontSize: 13,
      }}
    >
      <div style={{ fontWeight: 600 }}>{bannerText(cfg)}</div>
      {notice ? <div style={{ fontWeight: 400, marginTop: 2 }}>{notice}</div> : null}
    </div>
  );
}
