// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Card } from "./Card";

describe("Card", () => {
  it("renders children inside a .card container with no inline colors", () => {
    render(<Card><span>body</span></Card>);
    const card = screen.getByText("body").parentElement!;
    expect(card).toHaveClass("card");
    expect(card.getAttribute("style")).toBeNull();
  });
});
