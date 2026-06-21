import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { TextCard } from "../TextCard";
import { HeadingCard } from "../HeadingCard";
import { LinkCard } from "../LinkCard";

describe("TextCard", () => {
  it("renders markdown (heading, bold, list)", () => {
    render(<TextCard text={"# Title\n\n- **a**\n- b"} />);
    expect(screen.getByRole("heading", { name: "Title" })).toBeInTheDocument();
    expect(screen.getByText("a").tagName).toBe("STRONG");
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });
  it("renders nothing visible for empty text", () => {
    const { container } = render(<TextCard text="   " />);
    expect(container.textContent?.trim()).toBe("");
  });
});

describe("HeadingCard", () => {
  it("renders a single-line heading text", () => {
    render(<HeadingCard text="Quarterly KPIs" />);
    expect(screen.getByText("Quarterly KPIs")).toBeInTheDocument();
  });
});

describe("LinkCard", () => {
  it("renders an external link with label, opening a new tab", () => {
    render(<LinkCard url="https://example.com" label="Docs" />);
    const a = screen.getByRole("link", { name: "Docs" });
    expect(a).toHaveAttribute("href", "https://example.com");
    expect(a).toHaveAttribute("target", "_blank");
    expect(a).toHaveAttribute("rel", "noreferrer");
  });
  it("falls back to the url as label when no label", () => {
    render(<LinkCard url="https://example.com" />);
    expect(screen.getByRole("link", { name: "https://example.com" })).toBeInTheDocument();
  });
  it("shows a placeholder (no link) when url is empty", () => {
    render(<LinkCard url="" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText(/no url/i)).toBeInTheDocument();
  });
});
