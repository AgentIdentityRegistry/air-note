import type { HitDto } from "../api/engine";

/** Engine event kinds → human labels shown in the results list. */
export const KIND_LABEL: Record<string, string> = {
  memory: "Memory",
  page: "Dossier",
  file_ingested: "File",
};

export type Row = {
  id: string;
  kindLabel: string;
  sourcesLabel: string;
  score: string;
  text: string;
};

/** Map one recall hit to a display row (pure: kind→label, sources join, score 2dp). */
export const toRow = (h: HitDto): Row => ({
  id: h.event_id,
  kindLabel: KIND_LABEL[h.kind] ?? h.kind,
  sourcesLabel: h.sources.join(" + "),
  score: h.score.toFixed(2),
  text: h.text,
});
