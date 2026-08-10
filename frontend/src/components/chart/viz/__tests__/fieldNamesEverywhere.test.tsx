import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { cartesianDescriptor } from "../cartesian";
import { mapDescriptor } from "../map";
import { tableDescriptor } from "../table";
import { SIMPLE_DESCRIPTORS } from "../simple";
import type { VizControlProps, VizDescriptor } from "../types";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

/**
 * ONE COLUMN, ONE NAME, ON EVERY FIELD PICKER THERE IS.
 *
 * ── The defect ───────────────────────────────────────────────────────────────
 * #444 closed this between the table's checkbox list and the pivot's drag zones,
 * and put the rule in `config/field-names.ts` so it would live in one place. It
 * did not close it in the files that pick a field with a plain `<Select>`:
 * `cartesian.tsx` (X-axis, X metric, Bubble size, Series), `map.tsx` (Region,
 * Metric, Latitude, Longitude, Label) and `simple.tsx` (Goal from a metric).
 *
 * Driven on the kind deployment against the seeded `Demo Sales`, whose `region`
 * carries `verbose_name: "Khu vực"`, the pin map's Data tab showed BOTH at once:
 * the `Label` select offered `region`, and `Add tooltip columns…` — 78px below it
 * on the same panel, a `FieldZone`, already fixed — offered `Khu vực`. Same
 * column, same tab, two names, 78px apart. That is the state #444 describes as
 * the defect, still live in the files it declined to touch.
 *
 * ── Why this is one file and not four ────────────────────────────────────────
 * The failure is not "cartesian is wrong"; it is that a NEW picker gets written
 * the way its neighbours are written. A per-file test proves the sites it knows;
 * this one enumerates every control every descriptor declares and drives them, so
 * a picker added to any of them arrives already covered. Nothing here stubs a
 * `labelOf` — a stub passes just as happily when nothing threads the real dataset
 * column in, which is the mistake `cartesian.controls.test.tsx` already records
 * against the colour swatch.
 *
 * ── The two halves, and why both are needed ──────────────────────────────────
 * WHAT IT SAYS is swept generically below. WHAT IT STORES cannot be: shadcn's
 * `SelectItem` puts the value nowhere in the DOM (it goes into Radix's collection,
 * not onto the element), so the sweep can only read text — and a picker that had
 * started STORING "Khu vực" would sail through a text-only assertion while
 * silently writing a config no renderer resolves. That is the exact failure
 * `field-names.ts` opens by warning about, and seven bugs in this codebase have
 * come from keying on a display string. So `identity` below PICKS the renamed
 * option and asserts the machine name comes back out of `onChange`.
 *
 * ── What it does NOT pin ─────────────────────────────────────────────────────
 * jsdom lays nothing out, so this proves what the options SAY and not where they
 * sit. The 78px, and both dropdowns open on one panel, are browser numbers and
 * are in the PR body with screenshots.
 */

const col = (column_name: string, verbose_name = "", type_generic = "string"): DatasetColumn => ({
  column_name, data_type: type_generic === "numeric" ? "double" : "varchar", type_generic,
  is_temporal: false, ordinal: 0, expression: null, verbose_name, description: "",
  is_calculated: false, groupby: true, filterable: true, format: "", semantic_type: "",
});

const metric = (metric_name: string, verbose_name = ""): DatasetMetric =>
  ({
    metric_name, verbose_name, expression: "sum(1)", description: "", format: "",
    d3format: "", currency: "", ordinal: 0,
  } as unknown as DatasetMetric);

/** The seeded shape, reduced: one renamed column, one renamed metric, and plain ones beside them. */
const COLUMNS = [
  col("order_date", "", "temporal"),
  col("region", "Khu vực"),
  col("lat", "", "numeric"),
  col("lon", "", "numeric"),
  col("revenue", "", "numeric"),
];
const METRICS = [metric("total_revenue", "Doanh thu"), metric("order_count")];

/** Every field that has been renamed, machine name -> what a picker must say instead. */
const RENAMED: Record<string, string> = { region: "Khu vực", total_revenue: "Doanh thu" };

function props<T>(config: T, onChange: (p: Partial<T>) => void = () => {}): VizControlProps<T> {
  return {
    config, onChange, columns: COLUMNS, metrics: METRICS,
    resultColumns: COLUMNS.map((c) => ({ name: c.column_name, type: c.data_type })),
    rows: [], temporalUnits: {}, columnSettings: {}, unitRow: () => null,
  } as VizControlProps<T>;
}

/** Renders every control a descriptor declares for one config — what the builder's panel shows. */
function renderPanel<T>(d: VizDescriptor<T>, config: T, onChange?: (p: Partial<T>) => void) {
  return render(
    <>{d.controls.map((c) => createElement(c.render, { key: c.id, ...props(config, onChange) }))}</>,
  );
}

/**
 * Open every `<Select>` on the panel in turn and collect the visible text of every option.
 *
 * One at a time, because Radix mounts a Select's items only while it is open and unmounts them
 * on close — collecting them all in one pass would read an empty document.
 */
async function optionTexts(user: ReturnType<typeof userEvent.setup>): Promise<string[]> {
  const seen: string[] = [];
  for (const t of screen.queryAllByRole("combobox")) {
    await user.click(t);
    for (const o of screen.queryAllByRole("option")) seen.push((o.textContent ?? "").trim());
    await user.keyboard("{Escape}");
  }
  return seen;
}

/**
 * The assertion. The count guard is not ceremony: the first draft of this file read a
 * `data-value` attribute that `SelectItem` does not emit, so every case collected nothing and
 * every case passed its `for` loop vacuously. An empty walk must be a failure, loudly.
 */
function expectRenamed(texts: string[], where: string) {
  expect(texts.length, `${where}: no options were collected — the walk found nothing to assert on`)
    .toBeGreaterThan(0);
  for (const [machine, friendly] of Object.entries(RENAMED)) {
    expect(
      texts,
      `${where}: a picker offers "${machine}" while a FieldZone one control away calls the same ` +
        `field "${friendly}" — see field-names.ts.`,
    ).not.toContain(machine);
  }
}

describe("every field picker names a field the way its dataset does", () => {
  it.each([
    ["bar", { xAxis: "", yMetrics: [] }],
    ["scatter", { xAxis: "", yMetrics: [], xMetric: "revenue" }],
    ["line", { xAxis: "", yMetrics: [] }],
  ])("cartesian %s — X-axis, X metric, Bubble size and Series", async (kind, config) => {
    const user = userEvent.setup();
    renderPanel(cartesianDescriptor(kind as "bar"), config as never);
    expectRenamed(await optionTexts(user), `cartesian(${kind})`);
  });

  it("map, region mode — Region and Metric", async () => {
    const user = userEvent.setup();
    renderPanel(mapDescriptor(), { mapKey: "us_states", type: "region" as const });
    expectRenamed(await optionTexts(user), "map(region)");
  });

  it("map, pin mode — Latitude, Longitude and Label", async () => {
    const user = userEvent.setup();
    renderPanel(mapDescriptor(), { mapKey: "us_states", type: "pin" as const });
    expectRenamed(await optionTexts(user), "map(pin)");
  });

  it("progress — Goal from a metric", async () => {
    const user = userEvent.setup();
    const d = SIMPLE_DESCRIPTORS.find((x) => x.key === "progress")!;
    renderPanel(d as VizDescriptor<unknown>, d.defaultConfig);
    expectRenamed(await optionTexts(user), "progress");
  });

  /**
   * The `fieldDescriptor`-driven vizzes (funnel, pie, sankey, gauge…) already read `verbose_name`
   * — they were the one of #444's three named leftovers with NO user-visible symptom. Pinned
   * anyway, because this change rewrites the helper they use and a silent regression there would
   * look exactly like nothing happening.
   */
  it("the descriptor-driven pickers keep the names they already had", async () => {
    const user = userEvent.setup();
    const d = SIMPLE_DESCRIPTORS.find((x) => x.key === "funnel")!;
    renderPanel(d as VizDescriptor<unknown>, d.defaultConfig);
    expectRenamed(await optionTexts(user), "funnel");
  });

  /**
   * THE TABLE'S DATA TAB IS THE CONTROL GROUP — fixed by #444 and it must stay fixed. If this
   * goes red the shared helper broke rather than a call site.
   */
  /**
   * The table's Data-tab column list is GONE — it duplicated the Display tab's
   * `Add or remove columns`, which writes the same `config.columns`. The naming rule it was
   * asserting here did not go with it: the fold reaches the same fields through the same
   * `CheckList`, and `ChartExplore` hands it the same `labels`. That path is covered by
   * `table.checklist.test.tsx` ("shows a column's verbose name") and by
   * `ChartExplore.tableColumns.test.tsx`, which drives the real fold.
   *
   * Left as the METRICS half, which is still a Data-tab list of its own.
   */
  it("the table's Data-tab metric list still says Doanh thu", () => {
    // SELECTED: Metrics is a `FieldZone` now, so an unpicked field lives in the popover and only a
    // chip is on the panel. The rule under test is unchanged — a chip says the dataset's name.
    renderPanel(tableDescriptor(), { columns: [], metrics: ["total_revenue"] });
    expect(screen.getByText("Doanh thu")).toBeInTheDocument();
    expect(screen.queryByText("total_revenue")).toBeNull();
  });
});

describe("identity is still the machine name", () => {
  /**
   * The half a text sweep cannot see. Picking the renamed option must emit `region`, not
   * "Khu vực" — renaming a column in the dataset never rewrites a chart's stored configuration.
   */
  it("picking Khu vực on the map's Label select stores region", async () => {
    const user = userEvent.setup();
    const patches: Partial<{ label?: string }>[] = [];
    renderPanel(mapDescriptor(), { mapKey: "us_states", type: "pin" as const },
      (p) => patches.push(p as { label?: string }));
    await user.click(screen.getByLabelText("Label"));
    await user.click(screen.getByRole("option", { name: "Khu vực" }));
    expect(patches).toEqual([{ label: "region" }]);
  });

  it("picking Khu vực on a bar chart's X-axis stores region", async () => {
    const user = userEvent.setup();
    const patches: Partial<{ xAxis?: string }>[] = [];
    renderPanel(cartesianDescriptor("bar"), { xAxis: "", yMetrics: [] } as never,
      (p) => patches.push(p as { xAxis?: string }));
    await user.click(screen.getAllByRole("combobox")[0]);
    await user.click(screen.getByRole("option", { name: "Khu vực" }));
    expect(patches).toEqual([{ xAxis: "region" }]);
  });

  it("picking Doanh thu on progress's goal metric stores total_revenue", async () => {
    const user = userEvent.setup();
    const patches: Record<string, unknown>[] = [];
    const d = SIMPLE_DESCRIPTORS.find((x) => x.key === "progress")!;
    renderPanel(d as VizDescriptor<Record<string, unknown>>, d.defaultConfig as Record<string, unknown>,
      (p) => patches.push(p as Record<string, unknown>));
    await user.click(screen.getByLabelText("Goal from a metric"));
    await user.click(screen.getByRole("option", { name: "Doanh thu" }));
    expect(patches).toEqual([{ goalColumn: "total_revenue" }]);
  });
});
