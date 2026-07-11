import { type HitDto } from "../api/engine";
import { toRow } from "./recallView";

/**
 * Renders a list of recall hits (kind chip · sources · score, then the snippet) using the shared
 * `toRow` view helper — the same row shape the Brain search shows. Presentational only: the caller
 * owns the surrounding section, heading, and empty-state copy.
 */
export function HitList({ hits }: { hits: HitDto[] }) {
  return (
    <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
      {hits.map((h) => {
        const row = toRow(h);
        return (
          <li key={row.id} style={{ padding: "10px 0", borderBottom: "1px solid var(--border-soft)" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
              <span style={{
                fontSize: 11, fontWeight: 600, color: "var(--text-secondary)",
                background: "var(--surface-soft)", borderRadius: 4, padding: "2px 6px",
              }}>
                {row.kindLabel}
              </span>
              <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                {row.sourcesLabel}{row.sourcesLabel ? " · " : ""}score {row.score}
              </span>
            </div>
            <div style={{ fontSize: 14, lineHeight: 1.4 }}>{row.text}</div>
          </li>
        );
      })}
    </ul>
  );
}
