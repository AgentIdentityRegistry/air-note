/** The engine's recipe cap (`MAX_RECIPE_LEN`, graph.rs). Approximated client-side for fast feedback
 *  — the client check uses trimmed UTF-16 length while the engine checks the exact untrimmed UTF-8
 *  byte length, so they can diverge on multibyte/whitespace input; the engine is the authority and
 *  re-checks on add. */
export const MAX_RECIPE_LEN = 2048;

/** The raw New-mandate form fields. */
export type MandateFormInput = { target: string; sourceScope: string; recipe: string };

/** A pure validation result: ok, or a single human-readable error to show inline. */
export type MandateFormResult = { ok: true } | { ok: false; error: string };

/**
 * Validate the New-mandate form CLIENT-SIDE for fast feedback (the engine's grant-time guards are
 * still the authority — `add_mandate` re-checks recipe length, source count, write-grant, and the
 * read-root self-loop, surfacing a typed Rejected error the form also shows). Pure + deterministic.
 */
export function validateMandateForm(input: MandateFormInput): MandateFormResult {
  const target = input.target.trim();
  const sourceScope = input.sourceScope.trim();
  const recipe = input.recipe.trim();
  if (target === "") return { ok: false, error: "Pick a target file." };
  if (sourceScope === "") return { ok: false, error: "Pick a source folder." };
  if (recipe === "") return { ok: false, error: "Describe how to keep it in sync (the recipe)." };
  if (recipe.length > MAX_RECIPE_LEN) {
    return { ok: false, error: `The recipe is too long (max ${MAX_RECIPE_LEN} characters).` };
  }
  // A quick self-loop sanity check (the engine's segment-aware guard is the real one); catch the
  // obvious case where the target is the source scope itself.
  if (target === sourceScope) {
    return { ok: false, error: "The target must be outside the source folder." };
  }
  return { ok: true };
}
