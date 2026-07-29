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
});
