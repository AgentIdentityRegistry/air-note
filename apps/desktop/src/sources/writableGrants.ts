/** True iff there is at least one active root and every one of them is writable. */
export function allWritable(activeRoots: string[], writable: Set<string>): boolean {
  return activeRoots.length > 0 && activeRoots.every((r) => writable.has(r));
}
