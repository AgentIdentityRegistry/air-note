import type { ReasonerConfigDto } from "../api/engine";
import { bannerText, cloudActive } from "./reasonerView";

/** Persistent, non-dismissible indicator shown only while cloud reasoning is active. */
export function CloudEgressBanner({ cfg }: { cfg: ReasonerConfigDto | null }) {
  if (!cfg || !cloudActive(cfg)) return null;
  return (
    <div
      role="status"
      style={{
        marginTop: 12, padding: "8px 12px", borderRadius: 6,
        border: "1px solid var(--error)", color: "var(--error)",
        background: "var(--surface-soft)", fontSize: 13, fontWeight: 600,
      }}
    >
      {bannerText(cfg)}
    </div>
  );
}
