import { describe, it, expect } from "vitest";
import { validateMandateForm, MAX_RECIPE_LEN } from "./mandateForm";

describe("validateMandateForm", () => {
  const ok = { target: "/dest/synced.md", sourceScope: "/scope", recipe: "keep it synced" };

  it("accepts a complete form", () => {
    expect(validateMandateForm(ok)).toEqual({ ok: true });
  });

  it("rejects an empty target", () => {
    expect(validateMandateForm({ ...ok, target: "  " })).toEqual({ ok: false, error: "Pick a target file." });
  });

  it("rejects an empty source scope", () => {
    expect(validateMandateForm({ ...ok, sourceScope: "" })).toEqual({ ok: false, error: "Pick a source folder." });
  });

  it("rejects an empty recipe", () => {
    expect(validateMandateForm({ ...ok, recipe: "   " })).toEqual({ ok: false, error: "Describe how to keep it in sync (the recipe)." });
  });

  it("rejects a recipe over the engine cap", () => {
    const huge = "a".repeat(MAX_RECIPE_LEN + 1);
    expect(validateMandateForm({ ...ok, recipe: huge })).toEqual({
      ok: false, error: `The recipe is too long (max ${MAX_RECIPE_LEN} characters).`,
    });
  });

  it("rejects target == source scope (a self-loop the engine would reject anyway)", () => {
    expect(validateMandateForm({ target: "/scope/x.md", sourceScope: "/scope/x.md", recipe: "r" }))
      .toEqual({ ok: false, error: "The target must be outside the source folder." });
  });
});
