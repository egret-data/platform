import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
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
  it("does NOT render a javascript: URL as a link (XSS guard)", () => {
    render(<LinkCard url="javascript:alert(1)" label="x" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText(/no url/i)).toBeInTheDocument();
  });
  it("link card does not open a new tab in edit mode", () => {
    const { rerender } = render(<LinkCard url="https://x.com" label="X" editing={false} />);
    expect(screen.getByRole("link", { name: "X" })).toHaveAttribute("target", "_blank");
    rerender(<LinkCard url="https://x.com" label="X" editing />);
    expect(screen.getByRole("link", { name: "X" })).not.toHaveAttribute("target");
  });
});

describe("inline text/heading editing", () => {
  it("HeadingCard shows an editor bound to text when editing", () => {
    const onChange = vi.fn();
    render(<HeadingCard text="Hi" editing onChange={onChange} />);
    const input = screen.getByLabelText("heading text") as HTMLInputElement;
    expect(input.value).toBe("Hi");
    fireEvent.change(input, { target: { value: "Hello" } });
    expect(onChange).toHaveBeenCalledWith("Hello");
  });

  it("HeadingCard shows the display (no editor) when not editing", () => {
    render(<HeadingCard text="Hi" />);
    expect(screen.queryByLabelText("heading text")).toBeNull();
    expect(screen.getByText("Hi")).toBeInTheDocument();
  });

  it("TextCard editor shows raw text (not rendered markdown) when editing", () => {
    const onChange = vi.fn();
    render(<TextCard text="## Notes" editing onChange={onChange} />);
    const ta = screen.getByLabelText("text body") as HTMLTextAreaElement;
    expect(ta.value).toBe("## Notes");
    expect(screen.queryByRole("heading")).toBeNull();
    fireEvent.change(ta, { target: { value: "# New" } });
    expect(onChange).toHaveBeenCalledWith("# New");
  });

  it("TextCard renders markdown when not editing", () => {
    render(<TextCard text="# Title" />);
    expect(screen.queryByLabelText("text body")).toBeNull();
    expect(screen.getByRole("heading", { name: "Title" })).toBeInTheDocument();
  });
});
