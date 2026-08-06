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

  /**
   * THE DEAD "DISPLAY AS" REACHED DASHBOARD CARDS BY A DIFFERENT ROUTE, and it is the route most
   * likely to survive a gate written against the builder alone.
   *
   * `DashboardView` cannot see a card's result columns, so it builds the panel's column list out of
   * the chart's saved spec and gives every dimension `type: ""` (only metrics get a real
   * `"double"`). An empty type is neither numeric nor temporal, so EVERY dimension on EVERY card
   * lands in exactly the branch that used to offer Display as — a bar card's x-axis showed the
   * link select and its two template boxes even though nothing but a flat table can draw a link.
   *
   * The fixture keeps `type: ""` for that reason: giving these columns "varchar" would test a
   * shape this panel never actually receives.
   */
  const dashColumns = [{ name: "region", type: "" }, { name: "rev", type: "double" }];

  it("a bar card offers no Display as for its untyped dimension", async () => {
    const user = userEvent.setup();
    const barChart = {
      ...chart,
      params: {
        spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        cartesian: { xAxis: "region", yMetrics: ["rev"] },
      },
    } as unknown as Chart;
    render(<CardVizPanel chart={barChart} override={undefined} columns={dashColumns} rows={[]} onChange={vi.fn()} />);
    await user.click(screen.getByText("region"));
    // Vacuity guard: the accordion row really did expand, so "absent" is a statement about the
    // control rather than about an unopened body.
    expect(screen.getByText("Column title")).toBeInTheDocument();
    expect(screen.queryByLabelText("Display as")).not.toBeInTheDocument();
  });

  it("a TABLE card still offers it — the card's renderer is the one that reads it", async () => {
    const user = userEvent.setup();
    const tableChart = {
      ...chart,
      viz_type: "table",
      params: {
        spec: { metrics: [], dimensions: [], columns: ["region", "rev"], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
      },
    } as unknown as Chart;
    render(<CardVizPanel chart={tableChart} override={undefined} columns={dashColumns} rows={[]} onChange={vi.fn()} />);
    await user.click(screen.getByLabelText("settings region"));
    expect(screen.getByLabelText("Display as")).toBeInTheDocument();
  });
});

/**
 * THE SYNTHETIC `styleParams` HAS TO CARRY EVERY KEY `styleColumnsFor` INSPECTS.
 *
 * That function takes a real `ChartParams`; both editors hand it an object assembled by hand from
 * the effective config, and a key left out of that object is indistinguishable from a viz that is
 * not configured yet — `styleColumnsFor` filters on `undefined`, matches nothing, and
 * `SharedDisplaySections` hides the whole Formatting accordion behind its `styleColumns.length > 0`
 * gate. Silent in both directions.
 *
 * IT HAS NOW HAPPENED TWICE. The pie's `styleColumnsFor` arm was widened to include its dimensions,
 * the builder route's object grew a `pie` key, and this one did not — so a pie card could be
 * formatted from the chart editor and not from the dashboard. The map's arm, added in the same PR
 * as this test, would have been the second: it narrows the map to its metric, and narrowing against
 * a key that is not present hides the section outright.
 *
 * FOUND IN THE BROWSER on the builder route, which is the only reason it was found at all — the
 * unit tests around `styleColumnsFor` construct a real `ChartParams` and so cannot see a missing
 * key in either editor's synthetic one. These two render the PANEL, which is what closes that seam.
 */
describe("CardVizPanel — the Formatting section reaches the viz types that narrow", () => {
  const mapChart = {
    ...chart,
    viz_type: "map",
    params: {
      version: 2,
      spec: { metrics: ["rev"], dimensions: ["state"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
      settings: { "map.region": "state", "map.metric": "rev" },
    },
  } as unknown as Chart;
  const mapColumns = [{ name: "state", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }];

  it("offers the map's metric — and not its region, which is a geojson key", () => {
    render(<CardVizPanel chart={mapChart} override={undefined} columns={mapColumns} rows={[]} onChange={vi.fn()} />);
    expect(screen.getByText("Formatting")).toBeInTheDocument();
    // The accordion's rows are buttons named for the column they open.
    expect(screen.getByRole("button", { name: /^rev/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^state/ })).toBeNull();
  });

  // The sibling omission, fixed in the same line. A pie formats its dimensions AND its metric.
  it("offers the pie's dimensions and its metric", () => {
    const pieChart = {
      ...chart,
      viz_type: "pie",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "pie.dimensions": ["region"], "pie.metric": "rev" },
      },
    } as unknown as Chart;
    render(
      <CardVizPanel chart={pieChart} override={undefined}
        columns={[{ name: "region", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }]} rows={[]} onChange={vi.fn()} />,
    );
    expect(screen.getByRole("button", { name: /^region/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^rev/ })).toBeInTheDocument();
  });
});
