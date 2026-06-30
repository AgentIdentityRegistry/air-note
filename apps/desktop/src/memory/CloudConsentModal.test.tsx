// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { CloudConsentModal } from "./CloudConsentModal";

describe("CloudConsentModal", () => {
  it("blocks Enable until the box is checked, then calls onConfirm", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(<CloudConsentModal provider="anthropic" onConfirm={onConfirm} onCancel={() => {}} />);
    const enable = screen.getByRole("button", { name: /enable cloud reasoner/i });
    expect(enable).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(enable).toBeEnabled();
    fireEvent.click(enable);
    await waitFor(() => expect(onConfirm).toHaveBeenCalledOnce());
  });

  it("shows the classified error and stays open when enable rejects", async () => {
    const onConfirm = vi.fn().mockRejectedValue("cloud reasoner auth_rejected (HTTP 401)");
    render(<CloudConsentModal provider="anthropic" onConfirm={onConfirm} onCancel={() => {}} />);
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /enable cloud reasoner/i }));
    expect(await screen.findByText(/auth_rejected \(HTTP 401\)/)).toBeInTheDocument();
  });

  it("Cancel calls onCancel", () => {
    const onCancel = vi.fn();
    render(<CloudConsentModal provider="anthropic" onConfirm={vi.fn()} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
