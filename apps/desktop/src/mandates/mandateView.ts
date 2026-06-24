import type { MandateDto, MandateWriteDto } from "../api/engine";

/** A display row for one active mandate. */
export type MandateRow = {
  id: string;
  targetName: string;
  targetFolder: string;
  sourceScope: string;
  recipe: string;
  grantedAt: string;
};

/** A display row for one Mandate-activity entry (an auto-applied write). */
export type ActivityRow = {
  fileWrittenId: string;
  fileName: string;
  writtenAt: string;
  canUndo: boolean;
  label: string;
};

/** Split a canonical path into (basename, folder); folder is "" when there is no separator. */
function splitPath(path: string): { name: string; folder: string } {
  const slash = path.lastIndexOf("/");
  return slash >= 0 ? { name: path.slice(slash + 1), folder: path.slice(0, slash) } : { name: path, folder: "" };
}

/** Map a mandate DTO to a display row (pure: path split + pass-through). */
export function toMandateRow(m: MandateDto): MandateRow {
  const { name, folder } = splitPath(m.target);
  return {
    id: m.mandate_grant_id,
    targetName: name,
    targetFolder: folder,
    sourceScope: m.source_scope,
    recipe: m.recipe,
    grantedAt: m.granted_at,
  };
}

/** Map a mandate-write DTO to an activity row. `undone` disables Undo and relabels. */
export function toActivityRow(w: MandateWriteDto): ActivityRow {
  const { name } = splitPath(w.target);
  return {
    fileWrittenId: w.file_written_id,
    fileName: name,
    writtenAt: w.written_at,
    canUndo: !w.undone,
    label: w.undone ? "Undone" : "Synced",
  };
}
