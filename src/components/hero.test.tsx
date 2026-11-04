import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Hero } from "./hero";

describe("Hero", () => {
  it("renders the product name as a level-1 heading", () => {
    render(<Hero />);
    expect(
      screen.getByRole("heading", { level: 1, name: /qinari/i }),
    ).toBeInTheDocument();
  });

  it("links to the quick start section and the GitHub repository", () => {
    render(<Hero />);
    expect(screen.getByRole("link", { name: "Quick start" })).toHaveAttribute(
      "href",
      "#quickstart",
    );
    expect(
      screen.getByRole("link", { name: "View on GitHub" }),
    ).toHaveAttribute("href", "https://github.com/AlbiFahriiza/qinari");
  });
});
