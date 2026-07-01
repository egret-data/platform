import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { CellOutputRenderer } from "../CellOutputRenderer";

describe("CellOutputRenderer", () => {
  it("renders stream text", () => {
    render(<CellOutputRenderer outputs={[{ type: "stream", name: "stdout", text: "hello" }]} />);
    expect(screen.getByText(/hello/)).toBeInTheDocument();
  });
  it("renders an error traceback", () => {
    render(<CellOutputRenderer outputs={[{ type: "error", ename: "ValueError", evalue: "bad", traceback: ["Trace", "line"] }]} />);
    expect(screen.getByText(/ValueError/)).toBeInTheDocument();
  });
  it("renders an execute_result png as an img", () => {
    render(<CellOutputRenderer outputs={[{ type: "execute_result", data: { "image/png": "iVBORw0KGgo=" } }]} />);
    expect(screen.getByRole("img")).toHaveAttribute("src", expect.stringContaining("data:image/png;base64,iVBORw0KGgo="));
  });
  it("sanitizes text/html output", () => {
    render(<CellOutputRenderer outputs={[{ type: "execute_result", data: { "text/html": "<b>ok</b><script>alert(1)</script>" } }]} />);
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });
  it("shows a truncated marker", () => {
    render(<CellOutputRenderer outputs={[{ type: "truncated", dropped: 3 }]} />);
    expect(screen.getByText(/truncated/i)).toBeInTheDocument();
  });
});
