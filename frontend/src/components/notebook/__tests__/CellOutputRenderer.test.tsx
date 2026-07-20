import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi } from "vitest";
import { CellOutputRenderer } from "../CellOutputRenderer";
import type { CellOutput } from "@/lib/kernel/protocol";

// Mock plotly.js so the lazy PlotlyOutput import resolves without pulling the real bundle.
vi.mock("plotly.js-dist-min", () => ({
  default: { react: vi.fn(), purge: vi.fn(), Plots: { resize: vi.fn() } },
}));

describe("CellOutputRenderer", () => {
  it("renders stdout inline and collapses stderr under a Logs disclosure", async () => {
    const outputs: CellOutput[] = [
      { type: "stream", name: "stdout", text: "hello user" },
      { type: "stream", name: "stderr", text: "a warning line" },
    ];
    render(<CellOutputRenderer outputs={outputs} />);
    expect(screen.getByText(/hello user/)).toBeInTheDocument();
    expect(screen.queryByText(/a warning line/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /logs/i }));
    expect(screen.getByText(/a warning line/)).toBeInTheDocument();
  });

  it("renders an error with its ename and evalue", () => {
    const outputs: CellOutput[] = [
      { type: "error", ename: "ValueError", evalue: "bad", traceback: ["Traceback…"] },
    ];
    render(<CellOutputRenderer outputs={outputs} />);
    expect(screen.getByText(/ValueError: bad/)).toBeInTheDocument();
  });

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
  it("renders a plotly MIME bundle via the notebook-plotly container", async () => {
    const figure = { data: [{ type: "bar", x: ["a"], y: [1] }], layout: {} };
    render(
      <CellOutputRenderer
        outputs={[{ type: "display_data", data: { "application/vnd.plotly.v1+json": figure } }]}
      />,
    );
    expect(await screen.findByTestId("notebook-plotly")).toBeInTheDocument();
  });
});
