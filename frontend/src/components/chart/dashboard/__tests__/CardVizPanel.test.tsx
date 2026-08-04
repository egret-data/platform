import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CardVizPanel } from "../CardVizPanel";
import type { Chart } from "@/api/charts";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] }, cartesian: { yMetrics: ["rev"] } } } as unknown as Chart;

describe("CardVizPanel", () => {
  it("Reset clears the override", () => {
    const onChange = vi.fn();
    render(<CardVizPanel chart={chart} override={{ cartesian: { stacked: true } }} columns={[{ name: "rev", type: "DOUBLE" }]} rows={[]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "reset visualization" }));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  // The bug lives in this panel's merge, not in the control: PieDisplaySection already emits
  // `{ showLegend: undefined }` for Auto, but `patch` spreads the descriptor's settings over
  // what was already stored, and a spread cannot express a removal — so choosing Auto used to
  // write the old explicit `true` straight back, leaving Auto reachable but unsavable.
  it("choosing Auto for a setting the chart stores explicitly clears it from the override", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const pieChart = {
      ...chart,
      viz_type: "pie",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "pie.dimensions": ["region"], "pie.metric": "rev", "pie.showLegend": true },
      },
    } as unknown as Chart;

    render(
      <CardVizPanel chart={pieChart} override={{ "pie.showLegend": true }}
        columns={[{ name: "region", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }]} rows={[]} onChange={onChange} />,
    );
    await user.click(screen.getByLabelText("Legend"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));

    const next = onChange.mock.calls.at(-1)![0] as Record<string, unknown>;
    expect("pie.showLegend" in next).toBe(false);
  });

  /**
   * THE UNITS THE CARD'S ROWS WERE BUCKETED BY HAVE TO REACH THE PANEL.
   *
   * A dashboard has no unit picker at all — `unitRow` here is `() => null` — so the only place the
   * answer exists is the saved `spec.temporal_units`, which is also what `ChartRenderer` builds the
   * card's chart from. Under `day_of_week` the pie aggregates on "Mon"/"Wed", so a panel that did
   * not receive the units listed the raw `1`/`3` and every switch, rename and swatch on it wrote a
   * key the chart never looks up.
   *
   * The fixture is chosen so it can tell the two apart: neither raw value is its own label. A
   * truncation unit (`month`) could not detect this at all.
   */
  it("lists a bucketed pie's slices by the key the card's chart drew, not the raw column value", () => {
    const dowChart = {
      ...chart,
      viz_type: "pie",
      params: {
        version: 2,
        spec: {
          metrics: ["rev"], dimensions: ["dow"], columns: [], filters: [], orderby: [],
          row_limit: 100, rollup_groups: [], temporal_units: { dow: "day_of_week" },
        },
        settings: { "pie.dimensions": ["dow"], "pie.metric": "rev" },
      },
    } as unknown as Chart;

    render(
      <CardVizPanel chart={dowChart} override={undefined}
        columns={[{ name: "dow", type: "BIGINT" }, { name: "rev", type: "DOUBLE" }]}
        rows={[[3, 20], [1, 40]]} onChange={vi.fn()} />,
    );

    expect(screen.getByLabelText("Show Mon")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Wed")).toBeInTheDocument();
    expect(screen.queryByLabelText("Show 1")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Show 3")).not.toBeInTheDocument();
  });
});
