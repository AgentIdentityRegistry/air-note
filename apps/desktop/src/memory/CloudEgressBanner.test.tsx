// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { CloudEgressBanner } from "./CloudEgressBanner";
import type { ReasonerConfigDto } from "../api/engine";

const cfg: ReasonerConfigDto = { mode: "cloud", provider: "anthropic", model: "m", base_url: null, ready: true };

describe("CloudEgressBanner", () => {
  it("shows the egress warning when cloud is active", () => {
    render(<CloudEgressBanner cfg={cfg} />);
    expect(screen.getByText(/context leaves this device/i)).toBeInTheDocument();
  });
  it("renders nothing when local or not ready", () => {
    const { container, rerender } = render(<CloudEgressBanner cfg={{ ...cfg, mode: "local" }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<CloudEgressBanner cfg={{ ...cfg, ready: false }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<CloudEgressBanner cfg={null} />);
    expect(container).toBeEmptyDOMElement();
  });
  it("discloses the file-derived snippet count when a cloud tick has run", () => {
    render(<CloudEgressBanner cfg={cfg} taintedSnippets={3} />);
    expect(screen.getByText(/context leaves this device/i)).toBeInTheDocument();
    expect(screen.getByText(/3 snippets from your ingested files/i)).toBeInTheDocument();
  });
  it("omits the count line until a cloud tick runs (null), keeping the egress warning", () => {
    render(<CloudEgressBanner cfg={cfg} taintedSnippets={null} />);
    expect(screen.getByText(/context leaves this device/i)).toBeInTheDocument();
    expect(screen.queryByText(/ingested files/i)).not.toBeInTheDocument();
  });
});
