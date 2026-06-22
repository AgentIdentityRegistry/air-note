import type { IngestReportDto } from "../api/engine";

export function ingestSummary(r: IngestReportDto): string {
  return [
    `${r.ingested} added`,
    `${r.superseded} updated`,
    `${r.deduped} unchanged`,
    `${r.skipped.length} skipped`,
    `${r.failed.length} failed`,
  ].join(" · ");
}
