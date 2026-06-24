import type { FileRecordDto } from "../api/engine";
import type { SearchResult } from "./types";

/** Last path segment, handling both posix (/) and windows (\) separators. */
export function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** Pure client-side filter over the already-loaded file list (name/path). */
export function filterFiles(files: FileRecordDto[], query: string, cap = 5): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return files
    .filter((f) => f.canonical_path.toLowerCase().includes(q))
    .slice(0, cap)
    .map((f) => ({
      id: `file:${f.file_event_id}`,
      kind: "file" as const,
      title: basename(f.canonical_path),
      snippet: f.canonical_path,
      target: { view: "settings" as const },
    }));
}
