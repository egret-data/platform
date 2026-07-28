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
    unitRow: () => null,
    tab: "display" as const,
  };

  it("shows Series, Axes and Format headings", () => {
    render(<StudioPanel {...props} />);
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings).toEqual(expect.arrayContaining(["Series", "Axes", "Format"]));
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
