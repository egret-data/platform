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
   * An empty column type is neither numeric nor temporal, so a column carrying one lands in exactly
   * the branch that used to offer Display as — a bar card's x-axis showed the link select and its
   * two template boxes even though nothing but a flat table can draw a link. What these two tests
   * pin is that the gate is on the VIZ TYPE, not on the column's: the bar refuses it and the table
   * offers it, for the same column list.
   *
   * THE FIXTURE'S PROVENANCE CHANGED, and the note it used to carry is preserved here because the
   * change is the point. `type: ""` used to be what `DashboardView` handed this panel for EVERY
   * dimension on every card — it reconstructed the column list from `params.spec` and had no result
   * to type it from. That is what the dashcard-panel work replaced: the route now passes the card's
   * own result columns, so a dimension arrives as `date`/`varchar` like it does in the builder. The
   * spec-derived list survives only as the fallback for a card whose query has not landed or has
   * errored, which is still a shape this panel receives — so the fixture stays, now testing the
   * degenerate arm on purpose rather than the everyday one by accident.
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

  /**
   * THE AXES CONTROLS WERE DROPPED ENTIRELY, and by an omission rather than a decision.
   *
   * `StudioPanel`'s `tab="display"` filter is `c.section === "display" && c.group !== AXES_GROUP` —
   * it EXCLUDES the Axes group, because on the builder route that group is promoted out of the
   * Display tab into a tab of its own (`tab="axes"`). This panel asked for `tab="display"` and
   * nothing else, so on every viz that declares axes controls — the waterfall, the box plot and the
   * whole cartesian family — the exclusion had no matching inclusion anywhere and the group fell on
   * the floor. On a dashboard you could not override an axis label, hide gridlines, set a y-range or
   * rotate x labels, all of which the builder offers for the same chart.
   *
   * Metabase's dashcard modal keeps a full Axes tab (checked on its waterfall and its box plot), so
   * this is a straight defect on both counts: against Metabase, and against egret's own builder.
   *
   * The sidebar has no tabs, so the group is rendered as a labelled SECTION in the same scroll —
   * which is what the Display tab does with every other `group` a descriptor declares.
   */
  it("a waterfall card offers the Axes controls its builder route does", () => {
    const wfChart = {
      ...chart,
      viz_type: "waterfall",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["month"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "waterfall.xAxis": "month", "waterfall.metric": "rev" },
      },
    } as unknown as Chart;
    render(<CardVizPanel chart={wfChart} override={undefined}
      columns={[{ name: "month", type: "date" }, { name: "rev", type: "double" }]} rows={[]} onChange={vi.fn()} />);

    // The two `role="group"` blocks `ChartAxesSection` renders, named for the axis they own.
    expect(screen.getByRole("group", { name: "X-axis" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Y-axis" })).toBeInTheDocument();
    // …and a control from inside each, so this is about the settings and not just the headings.
    expect(screen.getByLabelText("X label")).toBeInTheDocument();
    expect(screen.getByLabelText("Number of tick marks")).toBeInTheDocument();
  });

  // A viz with no axes controls must not grow an empty "Axes" heading. The pie is the case the
  // builder route guards with `hasAxesControls`, and the same guard has to hold here.
  it("a pie card grows no Axes section, because a pie has no axes", () => {
    const pieChart = {
      ...chart,
      viz_type: "pie",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "pie.dimensions": ["region"], "pie.metric": "rev" },
      },
    } as unknown as Chart;
    render(<CardVizPanel chart={pieChart} override={undefined}
      columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]} rows={[]} onChange={vi.fn()} />);
    expect(screen.queryByText("Axes")).toBeNull();
    expect(screen.queryByRole("group", { name: "X-axis" })).toBeNull();
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
