// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import ReflectPanel from "./ReflectPanel";
import * as api from "../api/integrations";

vi.mock("../api/integrations", async (importOriginal) => {
  // A spread-over-actual mock: keep the real module (so new api exports survive the mock) and stub
  // only the two reflect wrappers the panel uses, so no Tauri `invoke` is touched.
  const actual = await importOriginal<typeof import("../api/integrations")>();
  return {
    ...actual,
    reflectEnabled: vi.fn(),
    setReflectEnabled: vi.fn(),
  };
});

describe("ReflectPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    // The mount read defaults to OFF so tests that don't care never trip the fail-closed path.
    vi.mocked(api.reflectEnabled).mockResolvedValue(false);
    vi.mocked(api.setReflectEnabled).mockResolvedValue(undefined);
  });

  it("the reflect toggle reflects the engine flag and toggling calls setReflectEnabled", async () => {
    vi.mocked(api.reflectEnabled).mockResolvedValue(true);
    vi.mocked(api.setReflectEnabled).mockResolvedValue(undefined);
    render(<ReflectPanel />);
    const toggle = await screen.findByRole("checkbox", { name: /reflect on recently-missed topics/i });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(api.setReflectEnabled).toHaveBeenCalledWith(false));
  });

  it("fails closed to OFF when the engine flag can't be read", async () => {
    // An unreadable flag (not onboarded / daemon down) must render the toggle OFF, never crash — the
    // daemon's flag is the sole truth and "off" is the safe default to show.
    vi.mocked(api.reflectEnabled).mockRejectedValue("daemon down");
    render(<ReflectPanel />);
    const toggle = await screen.findByRole("checkbox", { name: /reflect on recently-missed topics/i });
    expect(toggle).not.toBeChecked();
  });

  it("re-reads the daemon's truth (stays OFF) when the write fails, never assuming success", async () => {
    // The panel's other headline safety property: a failed write must re-read the daemon, never assume.
    // Mount reads OFF; the write REJECTS, so the optimistic setEnabled(next) is skipped and the catch
    // re-reads reflectEnabled() — which still reports false (the daemon never flipped). The toggle must
    // end UNCHECKED: re-read wins, optimism does not.
    vi.mocked(api.reflectEnabled).mockResolvedValue(false); // both the mount read AND the re-read
    vi.mocked(api.setReflectEnabled).mockRejectedValue("write failed");
    render(<ReflectPanel />);
    const toggle = await screen.findByRole("checkbox", { name: /reflect on recently-missed topics/i });
    expect(toggle).not.toBeChecked();

    fireEvent.click(toggle); // attempt to turn ON
    await waitFor(() => expect(api.setReflectEnabled).toHaveBeenCalledWith(true));
    expect(api.setReflectEnabled).toHaveBeenCalledTimes(1);
    // Re-read won over optimism: the toggle stays UNCHECKED (never shows a state the daemon rejected).
    await waitFor(() => expect(toggle).not.toBeChecked());
  });
});
