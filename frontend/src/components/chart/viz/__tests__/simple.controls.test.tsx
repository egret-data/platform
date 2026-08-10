import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { SIMPLE_DESCRIPTORS } from "../simple";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { funnelSteps } from "@/components/chart/echarts/options/funnel";
import { temporalLabel } from "@/components/chart/temporal-units";
import type { ColumnFormat } from "@/components/chart/format-types";
import type { VizControlProps } from "../types";

/**
 * THE SLICE LIST READS IN THE ORDER THE PIE DRAWS.
 *
 * `PieDisplayControl` used to hand the panel the raw DISTINCT dimension values, in row order, with
 * no measure at all — so the list could neither sort by value (the pie's own order) nor reproduce
 * the "Other" fold. Both are asserted here, through the descriptor's own control and against
 * `buildPieOption`'s actual output, rather than against a restatement of the rule: a panel test that
 * is handed a pre-ordered `slices` array proves the section renders what it is given and says
 * nothing about what the app gives it. (PR 02 found the same gap on the cartesian side.)
 */
describe("the pie's slice list, through the descriptor's own control", () => {
  const pieDescriptor = SIMPLE_DESCRIPTORS.find((d) => d.key === "pie")!;
  const control = pieDescriptor.controls.find((c) => c.section === "display")!;
  const cols = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];

  /**
   * THE FIXTURE IS NOT PRE-SORTED, and that is the whole point of it: rows arrive
   * Gizmo, Widget, Gadget, Doohickey while the pie draws Widget, Gadget, Gizmo, Doohickey. A
   * fixture whose rows already arrive value-descending cannot detect a sorting bug at all — it
   * passes against the broken control, which simply echoed the row order back.
   *
   * The values are also chosen so the fold has something to bite on: at a 26% threshold Gizmo (20%)
   * and Doohickey (10%) are both under it, which is 2+ slices and so a real "Other" bucket, while at
   * 15% only Doohickey is — and `buildSlices` declines to fold a single slice (Metabase's merge-2+
   * rule).
   */
  const rows: unknown[][] = [["Gizmo", 20], ["Widget", 40], ["Gadget", 30], ["Doohickey", 10]];
  const RAW_ORDER = ["Gizmo", "Widget", "Gadget", "Doohickey"];

  const base = { dimensions: ["category"], metric: "m" };

  const renderPanel = (cfg: Record<string, unknown> = {}) =>
    render(createElement(control.render, {
      config: { ...base, ...cfg },
      onChange: () => {},
      columns: [], metrics: [],
      resultColumns: cols,
      rows,
      temporalUnits: {},
      columnSettings: {},
      unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));

  /** Ring 0 exactly as the chart draws it: the sunburst's top-level data, in array order. */
  const ring0 = (cfg: Record<string, unknown> = {}) =>
    (buildPieOption(cols, rows, { ...base, ...cfg }) as unknown as {
      series: { data: { name: string; itemStyle: { color: string } }[] }[];
    }).series[0].data;
  const drawnOrder = (cfg: Record<string, unknown> = {}) => ring0(cfg).map((d) => d.name);
  const drawnColors = (cfg: Record<string, unknown> = {}) =>
    Object.fromEntries(ring0(cfg).map((d) => [d.name, d.itemStyle.color.toLowerCase()]));

  /** The panel's rows, in the order they are rendered — read off the one control every row has. */
  const panelRows = () =>
    screen.getAllByLabelText(/^reorder /).map((el) => el.getAttribute("aria-label")!.slice(8));
  const swatch = (name: string) =>
    (screen.getByLabelText(`color ${name}`) as HTMLInputElement).value.toLowerCase();

  it("lists the slices in the order the pie draws them, not the order the rows arrived", () => {
    const drawn = drawnOrder();
    expect(drawn).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    expect(drawn).not.toEqual(RAW_ORDER); // the fixture can tell the two apart
    renderPanel();
    expect(panelRows()).toEqual(drawn);
  });

  // "Sort by value" off is the pie drawing its rows in the order they arrived, and the list has to
  // follow the CHART rather than sort by value on its own account.
  it("follows the chart back to row order when Sort by value is off", () => {
    expect(drawnOrder({ sortRows: false })).toEqual(RAW_ORDER);
    renderPanel({ sortRows: false });
    expect(panelRows()).toEqual(RAW_ORDER);
  });

  // A stored manual position is applied on top of the chart's order, by both sides — so the rows
  // the user did not move stay value-descending underneath it.
  it("applies a stored sliceOrder on top of the drawn order, as the chart does", () => {
    const cfg = { sliceOrder: ["Doohickey"] };
    expect(drawnOrder(cfg)).toEqual(["Doohickey", "Widget", "Gadget", "Gizmo"]);
    renderPanel(cfg);
    expect(panelRows()).toEqual(drawnOrder(cfg));
  });

  /**
   * A FOLDED SLICE HAS NO ACTIONABLE ROW. Its colour, its name and its visibility are all decided by
   * the "Other" bucket now, so a row offering those three is a row of controls that do nothing.
   * Metabase renders nothing at all for a row it marked `isOther` (ChartSettingOrderedItems'
   * `renderItem` returns null).
   */
  it("drops a folded slice's row rather than leaving it with controls that do nothing", () => {
    const cfg = { sliceThreshold: 26 };
    expect(drawnOrder(cfg)).toEqual(["Widget", "Gadget", "Other"]); // 2 slices folded, so a bucket exists
    renderPanel(cfg);
    expect(panelRows()).toEqual(["Widget", "Gadget"]);
    for (const gone of ["Gizmo", "Doohickey"]) {
      expect(screen.queryByLabelText(`Show ${gone}`)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(`color ${gone}`)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(`label ${gone}`)).not.toBeInTheDocument();
    }
  });

  // …and the fold is reported, so the reader can see where the two missing categories went. It is
  // not a row: the bucket takes no curation on either side (the builder identifies it by a sentinel
  // that no stored key can carry).
  it("names the Other bucket and what went into it, without offering it as a row", () => {
    renderPanel({ sliceThreshold: 26 });
    expect(screen.getByText(/^Other/)).toHaveTextContent("Gizmo");
    expect(screen.getByText(/^Other/)).toHaveTextContent("Doohickey");
    expect(screen.queryByLabelText("Show Other")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("reorder Other")).not.toBeInTheDocument();
  });

  /** `buildSlices` refuses to fold a single sub-threshold slice — so the panel must not drop one
   *  either. A list that simply hid everything under the threshold would fail this. */
  it("keeps the row of a lone sub-threshold slice, which the chart does not fold", () => {
    const cfg = { sliceThreshold: 15 }; // only Doohickey (10%) is under it
    expect(drawnOrder(cfg)).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    renderPanel(cfg);
    expect(panelRows()).toEqual(drawnOrder(cfg));
  });

  /**
   * THE COLOUR CONSEQUENCE OF THE FOLD, which is what made this more than a cosmetic list bug.
   * A folded slice is not a ring-0 key, so its `sliceColors` pin never reaches `colorsForKeys` and
   * never consumes that colour — the chart hands it to whoever hashes onto it. The panel, listing
   * the folded slice as a live key, spent the slot and named a different colour for a slice that
   * was not folded at all.
   */
  it("does not spend a colour on a folded slice, so the survivors' swatches are the drawn ones", () => {
    const cfg = { sliceThreshold: 26, sliceColors: { Gizmo: "#f2a86f" } };
    const chart = drawnColors(cfg);
    expect(chart["Widget"]).toBe("#f2a86f"); // the premise: the pin freed nothing, Widget takes it
    renderPanel(cfg);
    for (const name of ["Widget", "Gadget"]) expect(swatch(name)).toBe(chart[name]);
  });

  /** The arrows walk the LISTED rows. Indexing them against a list that still counted the folded
   *  slices would leave the last row's ↓ live and swap it with a slice that has no row. */
  it("ends the arrows at the last listed row, not at the last slice in the data", () => {
    const onChange = vi.fn();
    render(createElement(control.render, {
      config: { ...base, sliceThreshold: 26 },
      onChange, columns: [], metrics: [], resultColumns: cols, rows,
      temporalUnits: {}, columnSettings: {}, unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));
    // Both rows carry a grip — a drag has no first/last to disable, unlike the ▲▼ pair this
    // replaced. What the drop WRITES is pinned on `reorderSeriesOrder` directly, because no test
    // here can drive a dnd drop.
    expect(screen.getByLabelText("reorder Widget")).not.toBeDisabled();
    expect(screen.getByLabelText("reorder Gadget")).not.toBeDisabled();
  });

  // A hidden slice is not drawn, but it must stay listed — its own switch is the only way back.
  it("still lists a hidden slice, which is the only route back to showing it", () => {
    renderPanel({ hiddenSlices: ["Widget"] });
    expect(panelRows()).toContain("Widget");
    expect((screen.getByLabelText("Show Widget") as HTMLInputElement).getAttribute("data-state"))
      .toBe("unchecked");
  });
});

/**
 * A TEMPORAL EXTRACTION UNIT — THE PANEL'S KEYS ARE THE CHART'S KEYS, OR THE PANEL EDITS NOTHING.
 *
 * Under `day_of_week` the pie AGGREGATES on `temporalLabel(unit, raw)`: "Wed" is the slice's key,
 * not the `3` sitting in the column (see `aggregateRing`). Every stored curation — `sliceColors`,
 * `sliceNames`, `hiddenSlices`, `sliceOrder` — is matched against that key, so a panel that keyed on
 * the raw column value wrote `hiddenSlices: ["3"]` against a chart that only ever asks about "Wed".
 * Nothing errored; the switch simply did nothing.
 *
 * The derivation was already right — `pieSliceRows` has been unit-aware since #392 — and the seam
 * that was NOT: the unit lives in `params.spec.temporal_units`, and `VizControlProps` carried no
 * temporal units at all, so `PieDisplayControl` had none to pass. These tests render the REAL
 * control out of the descriptor (and, below, through the real `StudioPanel`) so that a contract
 * that stops carrying the units fails here rather than passing on a hand-built config.
 */
describe("the pie's slice list under a temporal extraction unit", () => {
  const pieDescriptor = SIMPLE_DESCRIPTORS.find((d) => d.key === "pie")!;
  const control = pieDescriptor.controls.find((c) => c.section === "display")!;
  const cols = [{ name: "dow", type: "bigint" }, { name: "m", type: "double" }];

  /**
   * THE FIXTURE HAS TO BE ABLE TO TELL RAW FROM EXTRACTED, and that rules out most of the obvious
   * ones. `day_of_week` 3 -> "Wed" and 1 -> "Mon": neither raw value is a prefix, suffix or
   * stringification of its own label, so a panel keyed on the column and a panel keyed on the ring
   * cannot produce the same list. A TRUNCATION unit (`month`, `day`) could not detect this at all —
   * `temporalLabel` returns those values unchanged, so the two keyings agree by construction. The
   * last test in this block pins that, so nobody "simplifies" the fixture into a blind one.
   *
   * Not pre-sorted either, for the reason the block above states: Mon (40) draws before Wed (20)
   * while the rows arrive Wed first.
   */
  const rows: unknown[][] = [[3, 20], [1, 40]];
  const RAW_KEYS = rows.map((r) => String(r[0]));
  const temporalUnits = { dow: "day_of_week" };
  const base = { dimensions: ["dow"], metric: "m" };

  /** What the CHART is built from — the units arrive as `dimensionUnits`, which is what
   *  `ChartRenderer` forwards `params.spec.temporal_units` as. */
  const chartCfg = (cfg: Record<string, unknown> = {}) =>
    ({ ...base, dimensionUnits: temporalUnits, ...cfg });

  const drawnOrder = (cfg: Record<string, unknown> = {}) =>
    (buildPieOption(cols, rows, chartCfg(cfg)) as unknown as {
      series: { data: { name: string }[] }[];
    }).series[0].data.map((d) => d.name);

  const panelRows = () =>
    screen.getAllByLabelText(/^reorder /).map((el) => el.getAttribute("aria-label")!.slice(8));

  const renderPanel = (
    cfg: Record<string, unknown> = {},
    onChange: (patch: Record<string, unknown>) => void = () => {},
    units: Record<string, string> = temporalUnits,
  ) =>
    render(createElement(control.render, {
      config: { ...base, ...cfg },
      onChange,
      columns: [], metrics: [],
      resultColumns: cols,
      rows,
      temporalUnits: units,
      columnSettings: {},
      unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));

  it("draws a chart whose slice keys are nothing like the raw column values", () => {
    // The premise, asserted rather than assumed: this fixture CAN tell the two keyings apart.
    expect(drawnOrder()).toEqual(["Mon", "Wed"]);
    for (const raw of RAW_KEYS) expect(drawnOrder()).not.toContain(raw);
    expect(temporalLabel("day_of_week", 3)).toBe("Wed");
  });

  it("lists the slices by the word the chart keys on, not by the raw column value", () => {
    renderPanel();
    expect(panelRows()).toEqual(drawnOrder());
    // The failure being closed: the list used to read ["3", "1"] — row order, raw values.
    expect(panelRows()).not.toEqual(RAW_KEYS);
  });

  /**
   * THE ROUND TRIP, which is the only thing that proves the list is more than cosmetic: a hide
   * authored HERE has to reach the slice the chart DREW. Written by feeding the panel's own patch
   * back into `buildPieOption` rather than by asserting the patch's text, so a key that merely looks
   * plausible ("Wed" spelled some other way) still fails.
   */
  it("hands a hide authored in the panel to the slice the chart actually drew", () => {
    const onChange = vi.fn();
    renderPanel({}, onChange);
    fireEvent.click(screen.getByLabelText("Show Wed"));

    const patch = onChange.mock.calls.at(-1)![0] as { hiddenSlices: string[] };
    expect(patch.hiddenSlices).toEqual(["Wed"]);
    expect(drawnOrder(patch)).toEqual(["Mon"]); // the slice is gone from the chart
    // …and the switch is the way back, so the hidden slice stays listed under the same key.
    renderPanel(patch);
    expect(panelRows()).toContain("Wed");
  });

  /** The colour seam, for the same reason: a pin written under the panel's key has to be the colour
   *  the chart paints that slice. Keyed on the raw `1` it would have been spent on nothing. */
  it("pins a colour under a key the chart resolves", () => {
    const cfg = { sliceColors: { Mon: "#f2a86f" } };
    const drawn = (buildPieOption(cols, rows, chartCfg(cfg)) as unknown as {
      series: { data: { name: string; itemStyle: { color: string } }[] }[];
    }).series[0].data;
    expect(drawn.find((d) => d.name === "Mon")!.itemStyle.color.toLowerCase()).toBe("#f2a86f");
    renderPanel(cfg);
    expect((screen.getByLabelText("color Mon") as HTMLInputElement).value.toLowerCase()).toBe("#f2a86f");
  });

  /** The shell seam. The control gets its units from `VizControlProps`, which `StudioPanel` fills
   *  from its own prop — so this fails if the panel accepts the units and drops them on the floor,
   *  which rendering the control directly cannot catch. */
  it("carries the units through the real StudioPanel to the descriptor's control", () => {
    render(
      <StudioPanel
        descriptor={pieDescriptor}
        config={base}
        onConfigChange={() => {}}
        columns={[]} metrics={[]}
        resultColumns={cols}
        rows={rows}
        temporalUnits={temporalUnits}
        columnSettings={{}}
        unitRow={() => null}
        tab="display"
      />,
    );
    expect(panelRows()).toEqual(["Mon", "Wed"]);
  });

  /**
   * WHY THE FIXTURE ABOVE IS THE ONE THAT WORKS. A truncation unit's value is its own key on both
   * sides — `temporalLabel` returns it untouched — so a `month` fixture agrees whether or not the
   * units are threaded at all, and would have passed against the broken control. Pinned so the
   * discriminating fixture is not quietly traded for a blind one.
   */
  it("cannot be tested with a truncation unit, whose panel and chart already agree", () => {
    const monthCols = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
    const monthRows: unknown[][] = [["2026-02-01", 20], ["2026-01-01", 40]];
    const monthCfg = { dimensions: ["d"], metric: "m" };
    const withUnit = buildPieOption(monthCols, monthRows, { ...monthCfg, dimensionUnits: { d: "month" } });
    const keysOf = (o: unknown) =>
      (o as { series: { data: { name: string }[] }[] }).series[0].data.map((d) => d.name);
    // The raw strings ARE the keys, so nothing distinguishes a unit-aware panel from a blind one.
    expect(keysOf(withUnit)).toEqual(["2026-01-01", "2026-02-01"]);
    expect(keysOf(buildPieOption(monthCols, monthRows, monthCfg))).toEqual(keysOf(withUnit));
  });
});

/**
 * THE STEPS LIST NAMES A STEP THE WAY THE FUNNEL DRAWS IT.
 *
 * #423 gave the funnel's DIMENSION its own format — `funnelSteps` runs each step name through that
 * column's `column_settings`, so a month-bucketed funnel with a Date style draws "January 2026".
 * `FunnelStepsControl` went on listing `String(row[dimensionIndex])`, so the panel read
 * `2026-01-01` beside a chart reading "January 2026" and there was nothing on screen to connect
 * the two. The audit that recorded this could not measure it: the parity harness's funnel runs on
 * a TEXT step column, where the raw cell and the formatted cell are the same string, so the
 * fixtures below are the point — a text-dimension fixture is blind to this in exactly the way the
 * pie block above records for truncation units.
 *
 * WHAT DID NOT CHANGE, AND MUST NOT. The raw cell is the IDENTITY: `stepOrder` and `hiddenSteps`
 * store it, and `FunnelChart` drills on it. Keying the list on the drawn label would silently drop
 * a hand-made order the first time anyone picked a date style — the drill-on-display-strings defect
 * four chart families have now had. So the row SHOWS the label and STORES the raw, and both tests
 * below assert the pair together; either alone would pass against the wrong fix.
 *
 * The raw is also still on screen, muted, beside the label — `FormatSection` renders a renamed
 * column the same way (`column_title` first, then the real name). Without it the panel would name
 * a step one thing while every control on the row acts on another, which is the mismatch that made
 * this worth fixing rather than a second spelling of it.
 */
describe("the funnel's steps list, against the funnel the chart draws", () => {
  const funnel = SIMPLE_DESCRIPTORS.find((d) => d.key === "funnel")!;
  const control = funnel.controls.find((c) => c.id === "steps")!;
  const cols = [{ name: "d", type: "timestamp" }, { name: "cnt", type: "double" }];
  const rows: unknown[][] = [["2026-01-01", 10], ["2026-02-01", 5]];
  const base = { dimension: "d", metric: "cnt" };
  const columnSettings = { d: { date_style: "MMMM YYYY" } };

  const drawnNames = (cfg: Record<string, unknown> = {}) =>
    funnelSteps(cols, rows, { ...base, columnSettings, ...cfg }).map((s) => s.name);

  const renderPanel = (
    cfg: Record<string, unknown> = {},
    onChange: (patch: Record<string, unknown>) => void = () => {},
    formats: Record<string, ColumnFormat> = columnSettings,
  ) =>
    render(createElement(control.render, {
      config: { ...base, ...cfg },
      onChange,
      columns: [], metrics: [],
      resultColumns: cols,
      rows,
      temporalUnits: {},
      columnSettings: formats,
      unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));

  it("draws a funnel whose step names are nothing like the raw cells", () => {
    // The premise, asserted rather than assumed: this fixture CAN tell the two namings apart.
    expect(drawnNames()).toEqual(["January 2026", "February 2026"]);
    for (const raw of ["2026-01-01", "2026-02-01"]) expect(drawnNames()).not.toContain(raw);
  });

  it("shows the drawn label for each step, with the stored key beside it", () => {
    renderPanel();
    for (const drawn of drawnNames()) expect(screen.getByText(drawn)).toBeInTheDocument();
    // The key stays visible — the row's own controls act on it, so hiding it would leave the
    // panel naming a step one thing and editing another.
    for (const raw of ["2026-01-01", "2026-02-01"]) expect(screen.getByText(raw)).toBeInTheDocument();
  });

  /**
   * THE ROUND TRIP, and the half that makes the display change safe: a hide authored HERE has to
   * reach the step the chart DREW, via the raw key. Fed back into `funnelSteps` rather than
   * asserted as text, so a key that merely looks plausible ("January 2026") still fails.
   */
  it("stores the raw cell, so a hide authored in the panel reaches the drawn step", () => {
    const onChange = vi.fn();
    renderPanel({}, onChange);
    fireEvent.click(screen.getByLabelText("Show 2026-01-01"));
    const patch = onChange.mock.calls.at(-1)![0] as { hiddenSteps: string[] };
    expect(patch.hiddenSteps).toEqual(["2026-01-01"]);
    expect(drawnNames(patch)).toEqual(["February 2026"]);
  });

  /**
   * THE HAZARD EVERY ONE OF THESE FORMAT FIXES RECORDS. `FormatSection`'s patch spreads, so a
   * RENAME stores `{column_title: "Month"}` — truthy, and silent about dates. `formatValue` on a
   * temporal column would take `formatDate`'s default style and turn "2026-01-01" into
   * "January 1, 2026", i.e. a rename that quietly restyles every row. `funnelSteps` guards that
   * with `formatDimensionValue`; the panel has to use the SAME function, or the two disagree in
   * exactly the state this test names.
   */
  it("is not restyled by a format that says nothing about dates", () => {
    renderPanel({}, () => {}, { d: { column_title: "Month" } });
    expect(screen.getByText("2026-01-01")).toBeInTheDocument();
    expect(screen.queryByText("January 1, 2026")).toBeNull();
    // …and with the raw and the label identical there is no second, muted copy of it.
    expect(screen.getAllByText("2026-01-01")).toHaveLength(1);
  });

  /**
   * THE SHELL SEAM, the reason this was deferred out of #423 and the only part of it that is not
   * local to the control. `column_settings` belongs to the SHELL, not to any descriptor's config,
   * so the control can only read it if `VizControlProps` carries it — exactly as `temporalUnits`
   * is carried for the pie above. Rendering the control directly cannot catch a `StudioPanel` that
   * accepts the map and drops it on the floor, which is what this asserts.
   */
  it("carries column_settings through the real StudioPanel to the descriptor's control", () => {
    render(
      <StudioPanel
        descriptor={funnel}
        config={base}
        onConfigChange={() => {}}
        columns={[]} metrics={[]}
        resultColumns={cols}
        rows={rows}
        temporalUnits={{}}
        columnSettings={columnSettings}
        unitRow={() => null}
        tab="data"
      />,
    );
    expect(screen.getByText("January 2026")).toBeInTheDocument();
  });
});

/**
 * THE SANKEY'S THIRD DATA FIELD IS CALLED "VALUE".
 *
 * Metabase's `metricSetting("sankey.value", …)` is titled `Value` (`SankeyChart/chart-definition.ts`)
 * and egret's read "Metric". Behaviour was identical on both sides — this is the wording alone.
 * "Value" is also the better of the two here on its own merits: the number on a sankey link is the
 * WIDTH of a flow, not a metric being compared across categories, and the box plot next door already
 * calls the same kind of column "Value column".
 *
 * Asserted through the rendered control, on the text a reader sees, rather than against
 * `fields[2].label`: a label map is exactly the shape of thing that can be right in the descriptor
 * and never reach the panel, and `fieldDescriptor` builds the control from the field list rather
 * than the panel reading the field list itself.
 */
describe("the sankey's data fields, as the panel spells them", () => {
  const sankey = SIMPLE_DESCRIPTORS.find((d) => d.key === "sankey")!;
  const renderData = (id: string) =>
    render(createElement(sankey.controls.find((c) => c.id === id)!.render, {
      config: {},
      onChange: () => {},
      columns: [], metrics: [], resultColumns: [], rows: [],
      temporalUnits: {}, columnSettings: {}, unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));

  it("labels the value column Value, not Metric", () => {
    renderData("metric");
    expect(screen.getByText("Value")).toBeInTheDocument();
    expect(screen.queryByText("Metric")).not.toBeInTheDocument();
  });

  // Source and Target already match; asserted alongside so a later edit cannot quietly rename one
  // of the three while this file is only watching the other.
  it("still labels the two dimensions Source and Target", () => {
    renderData("source");
    expect(screen.getByText("Source")).toBeInTheDocument();
    renderData("target");
    expect(screen.getByText("Target")).toBeInTheDocument();
  });
});
