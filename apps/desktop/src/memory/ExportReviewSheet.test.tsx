// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { ExportReviewSheet } from "./ExportReviewSheet";

const NOTES = [{ id: "n1", text: "Aria prefers async standups" }];
const SESSIONS = [{ id: "s1", title: "Refactor auth" }];
const INGESTS = [{ id: "f1", path: "/Users/me/notes/plan.md" }];

function renderSheet(overrides: Partial<Parameters<typeof ExportReviewSheet>[0]> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  const utils = render(
    <ExportReviewSheet
      notes={NOTES}
      sessions={SESSIONS}
      ingests={INGESTS}
      exporting={false}
      error={null}
      onConfirm={onConfirm}
      onCancel={onCancel}
      {...overrides}
    />,
  );
  return { ...utils, onConfirm, onCancel };
}

/** The row (list item) whose text identifies one selected memory. */
const rowFor = (identity: string) => screen.getByText(identity).closest("li") as HTMLElement;

describe("ExportReviewSheet", () => {
  it("a stamped note row discloses that its FULL signed record ships", () => {
    renderSheet();
    const row = rowFor("Aria prefers async standups");

    // The whole signed event travels — text, when it was saved, its id, its chain link, the signature.
    expect(within(row).getByText(/full signed record/i)).toBeInTheDocument();
    expect(within(row).getByText(/exact text/i)).toBeInTheDocument();
    expect(within(row).getByText(/when you saved it/i)).toBeInTheDocument();
    expect(within(row).getByText(/brain’s signature/i)).toBeInTheDocument();
    // …including the owner's did: `Event.signed_by_did` has no `skip_serializing_if`, and
    // `canonical_bytes` strips only `hash`/`signature`, so the did is inside every stamped item's
    // `event_bytes`. An exhaustive list missing an item is the failure this sheet exists to prevent.
    expect(within(row).getByText(/your AIR name \(your did\)/i)).toBeInTheDocument();
    // The chain link is to the previous EVENT in the whole log — which may be a session capture or
    // an ingest, not a note — so the copy must not promise "the note before it".
    expect(within(row).getByText(/whatever you recorded just before it/i)).toBeInTheDocument();
    expect(within(row).queryByText(/the note before it/i)).toBeNull();
  });

  it("a session row discloses the FULL transcript plus the display fields that actually ship", () => {
    renderSheet();
    const row = rowFor("Refactor auth");

    expect(within(row).getByText(/FULL transcript/i)).toBeInTheDocument();
    // All SIX display fields core ships, each named — the frozen-key test in
    // `crates/bossclaw-core/src/log.rs` (`a_session_display_discloses_exactly_these_fields`) is what
    // makes a seventh field break a test instead of quietly going undisclosed here.
    expect(within(row).getByText(/its title/i)).toBeInTheDocument();
    expect(within(row).getByText(/which tool ran it/i)).toBeInTheDocument();
    expect(within(row).getByText(/when it started and ended/i)).toBeInTheDocument();
    expect(within(row).getByText(/rough size/i)).toBeInTheDocument();
    expect(within(row).getByText(/folder name/i)).toBeInTheDocument();
    // …and what never does.
    expect(within(row).getByText(/never the full path or the session id/i)).toBeInTheDocument();
    // The receiver's verifier labels it exactly this way.
    expect(within(row).getByText(/captured session, content only/i)).toBeInTheDocument();
    expect(within(row).getByText(/not independently verified/i)).toBeInTheDocument();
  });

  it("an ingested-file row discloses content-only disclosure and that the path never ships", () => {
    renderSheet();
    const row = rowFor("/Users/me/notes/plan.md");

    expect(within(row).getByText(/full extracted text/i)).toBeInTheDocument();
    expect(within(row).getByText(/content only/i)).toBeInTheDocument();
    expect(within(row).getByText(/path below is only shown here/i)).toBeInTheDocument();
    expect(within(row).getByText(/ingested file extract/i)).toBeInTheDocument();
    expect(within(row).getByText(/not independently verified/i)).toBeInTheDocument();
  });

  it("always states that the receiver can read everything, and that nothing is published", () => {
    renderSheet();
    expect(screen.getByText(/can read the plaintext of everything selected/i)).toBeInTheDocument();
    expect(screen.getByText(/does not publish anything/i)).toBeInTheDocument();
  });

  /**
   * `manifest.did`, `manifest.brain_verifying_key` and the whole binding (identity public key, did,
   * epoch, mint time) ride in EVERY bundle. Staying quiet about how strong that identity CLAIM is
   * is correct; staying quiet about the identifier travelling at all is under-disclosure — and the
   * consequence an owner would actually want is that the did is the SAME across exports.
   */
  it("discloses that a durable identifier and the public keys ship in every file", () => {
    renderSheet();
    const disclosure = screen.getByText(/your AIR name \(your did\) and your public keys/i);
    expect(disclosure).toBeInTheDocument();
    expect(disclosure).toHaveTextContent(/two receivers can tell two files came from the same person/i);
  });

  /**
   * The honesty bar this task inherits from Task 9: a sealed file proves the BRAIN wrote these bytes.
   * It does NOT prove which registered identity that brain belongs to — that needs a registry lookup
   * the receiver performs, offline verification cannot make the claim, and the shipped CLI refuses to.
   * The export sheet must not promise it either.
   */
  it("never claims the file proves which registered identity recorded the bytes", () => {
    const { container } = renderSheet();
    const copy = container.textContent ?? "";
    expect(copy).not.toMatch(/proves which registered identity/i);
    expect(copy).not.toMatch(/verified identity/i);
    // Two fixed phrases are a tripwire, not a rule: "proves who you are", "confirms your identity"
    // and "cryptographically verifies you" would all slip past them. Match the SHAPE of the claim.
    expect(copy).not.toMatch(/\b(prov|verif|confirm)\w*\s+(your|which|the)\s+identity/i);
    expect(copy).not.toMatch(/\b(prov|verif|confirm)\w*\s+who\s+you\s+are/i);
    // It says the honest thing instead: sealing is about authorship, not secrecy.
    expect(copy).toMatch(/does not keep it secret/i);
  });

  it("confirming passes the owner's description; cancelling exports nothing", () => {
    const { onConfirm, onCancel } = renderSheet();

    fireEvent.change(screen.getByLabelText(/note for whoever receives this/i), {
      target: { value: "standup context" },
    });
    fireEvent.click(screen.getByRole("button", { name: /export signed bundle/i }));
    expect(onConfirm).toHaveBeenCalledWith("standup context");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });

  it("shows a refusal verbatim and blocks a second click while exporting", () => {
    renderSheet({ error: "note n1 is not a current note", exporting: true });

    // The daemon's typed refusal is shown as-is — never softened into a generic 'export failed'.
    expect(screen.getByText("note n1 is not a current note")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sealing/i })).toBeDisabled();
  });

  it("uses design tokens only — no hardcoded colors in any inline style", () => {
    const { container } = renderSheet();
    const offenders = Array.from(container.querySelectorAll<HTMLElement>("[style]"))
      .map((el) => el.getAttribute("style") ?? "")
      .filter((style) => /#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(/i.test(style));
    expect(offenders).toEqual([]);
  });
});
