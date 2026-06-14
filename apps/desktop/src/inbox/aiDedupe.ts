/**
 * aiDedupe — bounded FIFO processed-envelope-id set for the AI inbox loop.
 *
 * `markProcessed` mutates the set in place (same reference) for efficiency,
 * then returns `{ set, fresh }`. The Task 6 controller can assign the returned
 * `set` back to its ref — the ref will already point to the mutated object, so
 * the assignment is a no-op in practice, but it keeps the call-site pattern
 * consistent with a functional style if the implementation ever changes.
 *
 * Eviction strategy: JS `Set<string>` preserves insertion order. When the size
 * exceeds DEDUPE_CAP after an insert, we evict the first (oldest) entry via
 * `set.values().next().value`. This is O(1) and requires no auxiliary structure.
 *
 * In-memory only — no persistence. D11 recency is the durable backstop against
 * post-restart re-drafting, so an evicted-then-re-seen id being treated as fresh
 * again is explicitly acceptable per spec.
 */

/** Maximum number of processed envelope ids retained before the oldest is evicted. */
export const DEDUPE_CAP = 1_000;

/** Returns a new, empty dedupe set. */
export function newDedupe(): Set<string> {
  return new Set<string>();
}

/**
 * Marks `id` as processed in `set`.
 *
 * - Mutates `set` in place and returns the same reference as `set`.
 * - `fresh` is `true` when `id` was not already present; `false` for a duplicate.
 * - When the set size exceeds DEDUPE_CAP after an insert, the oldest id is evicted (FIFO).
 */
export function markProcessed(
  set: Set<string>,
  id: string,
): { set: Set<string>; fresh: boolean } {
  if (set.has(id)) {
    return { set, fresh: false };
  }

  set.add(id);

  if (set.size > DEDUPE_CAP) {
    // JS Sets preserve insertion order; the first value is always the oldest.
    const oldest = set.values().next().value as string;
    set.delete(oldest);
  }

  return { set, fresh: true };
}
