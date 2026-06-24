// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Button } from "./Button";

describe("Button", () => {
  it("maps the primary variant to the primary token class", () => {
    render(<Button>Save</Button>);
    expect(screen.getByRole("button", { name: "Save" })).toHaveClass("floating-primary-btn");
  });

  it("maps the secondary variant to the secondary token class", () => {
    render(<Button variant="secondary">Cancel</Button>);
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveClass("secondary-btn");
  });

  it("merges an extra className and forwards onClick", () => {
    const onClick = vi.fn();
    render(<Button className="danger-btn" onClick={onClick}>Reset</Button>);
    const btn = screen.getByRole("button", { name: "Reset" });
    expect(btn).toHaveClass("floating-primary-btn");
    expect(btn).toHaveClass("danger-btn");
    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledOnce();
  });
});
