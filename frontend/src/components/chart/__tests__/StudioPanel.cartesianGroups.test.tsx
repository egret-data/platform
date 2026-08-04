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
 * The per-series line settings USED TO BE a separate `seriesLineStyle` control, extracted out of
 * the always-open "Series" section and marked `advanced` so an unbounded per-series grid of
 * Selects didn't swamp the panel — and this suite proved the disclosure still made it reachable.
 *
 * It moved. They now live in a drill-in popover hung off each series' own row, which is bounded by
 * construction (one button per row) and needs no disclosure at all — so the assertion moved with
 * it: NOT behind "More options", but one click from the row. The two halves of the old test still
 * matter and both survive here: nothing is on screen until asked for, and what is asked for is
 * genuinely reachable through the real descriptor + StudioPanel rather than through a stub.
 */
describe("StudioPanel: per-series line settings are one click from the series' own row", () => {
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

  it("keeps the four line settings off screen until a row's drill-in is opened", async () => {
    render(<StudioPanel {...props} />);
    // Not merely hidden behind a disclosure now — not rendered at all, for either series.
    for (const f of ["shape", "style", "size", "markers"]) {
      expect(screen.queryByLabelText(`${f} revenue`)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(`${f} cost`)).not.toBeInTheDocument();
    }
    // And opening every disclosure on the tab does not bring them back — the old route is gone,
    // which is the half of this that a "the new route works" assertion would not have caught.
    for (const trigger of screen.getAllByRole("button", { name: /more options/i })) {
      await userEvent.click(trigger);
    }
    expect(screen.queryByLabelText("style revenue")).not.toBeInTheDocument();

    await userEvent.click(screen.getByLabelText("settings revenue"));
    for (const f of ["shape", "style", "size", "markers"]) {
      expect(await screen.findByLabelText(`${f} revenue`)).toBeInTheDocument();
    }
    // One series at a time: the other row's settings stay where they were.
    expect(screen.queryByLabelText("style cost")).not.toBeInTheDocument();
  });
});
