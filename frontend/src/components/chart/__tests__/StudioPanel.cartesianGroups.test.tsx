import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StudioPanel } from "../StudioPanel";
import { cartesianDescriptor } from "../viz/cartesian";

/**
 * Proves the grouping lands end to end: mounting the real cartesian descriptor's Display
 * controls through the real StudioPanel (not a stub descriptor, unlike StudioPanel.test.tsx)
 * shows the named headings and that controls nested in a group — including ones behind the
 * "More options" disclosure — are actually reachable.
 */
describe("StudioPanel renders bar's grouped Display controls", () => {
  const props = {
    descriptor: cartesianDescriptor("bar"),
    config: { xAxis: "region", yMetrics: ["revenue"] },
    onConfigChange: vi.fn(),
    columns: [],
    metrics: [],
    resultColumns: [
      { name: "region", type: "varchar" },
      { name: "revenue", type: "double" },
    ],
    rows: [
      ["west", 1],
      ["east", 2],
    ],
    temporalUnits: {},
    unitRow: () => null,
    tab: "display" as const,
  };

  it("shows Series and Format headings — Axes has its own tab now", () => {
    render(<StudioPanel {...props} />);
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings).toEqual(expect.arrayContaining(["Series", "Format"]));
    expect(headings).not.toContain("Axes");
  });

  // The bar's real axis controls, in the tab that now owns them.
  it("puts the bar's axis controls in the Axes tab", () => {
    render(<StudioPanel {...props} tab="axes" />);
    expect(screen.getByLabelText("X axis scale")).toBeInTheDocument();
    expect(screen.getByLabelText("X axis lines and ticks")).toBeInTheDocument();
    expect(screen.getByLabelText("Y tick marks")).toBeInTheDocument();
    // …and not the Series ones.
    expect(screen.queryByLabelText("Legend")).toBeNull();
  });

  it("a non-advanced control inside a group is reachable — Legend under Series", () => {
    render(<StudioPanel {...props} />);
    expect(screen.getByLabelText("Legend")).toBeInTheDocument();
  });

  it("an advanced control becomes reachable once its group's disclosure opens", async () => {
    render(<StudioPanel {...props} />);
    expect(screen.queryByLabelText("Trend line")).not.toBeInTheDocument();
    for (const trigger of screen.getAllByRole("button", { name: /more options/i })) {
      await userEvent.click(trigger);
    }
    expect(screen.getByLabelText("Trend line")).toBeInTheDocument();
  });
});

/**
 * The per-series style/size control (seriesLineStyle) was extracted out of the always-open
 * "Series" section and marked advanced so an unbounded per-series list of Selects doesn't
 * swamp the panel. Extracting a control behind a disclosure is only safe if it stays
 * reachable — this proves it does, mounted through the real descriptor + StudioPanel for
 * "line", the kind it's declared for.
 */
describe("StudioPanel: per-series line style is reachable behind Series' More options", () => {
  const props = {
    descriptor: cartesianDescriptor("line"),
    config: { xAxis: "region", yMetrics: ["revenue", "cost"] },
    onConfigChange: vi.fn(),
    columns: [],
    metrics: [],
    resultColumns: [
      { name: "region", type: "varchar" },
      { name: "revenue", type: "double" },
      { name: "cost", type: "double" },
    ],
    rows: [
      ["west", 1, 1],
      ["east", 2, 2],
    ],
    temporalUnits: {},
    unitRow: () => null,
    tab: "display" as const,
  };

  it("hides style/size until Series' More options opens, then shows one row per series", async () => {
    render(<StudioPanel {...props} />);
    expect(screen.queryByLabelText("style revenue")).not.toBeInTheDocument();
    for (const trigger of screen.getAllByRole("button", { name: /more options/i })) {
      await userEvent.click(trigger);
    }
    expect(screen.getByLabelText("style revenue")).toBeInTheDocument();
    expect(screen.getByLabelText("size revenue")).toBeInTheDocument();
    expect(screen.getByLabelText("style cost")).toBeInTheDocument();
    expect(screen.getByLabelText("size cost")).toBeInTheDocument();
  });
});
