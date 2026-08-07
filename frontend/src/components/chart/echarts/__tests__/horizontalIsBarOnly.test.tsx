import { describe, it, expect } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { buildCartesianOption, isHorizontal, type CartesianBuildConfig } from "../options/cartesian";
import { CartesianAxesTab, CartesianDisplaySection } from "@/components/chart/settings/CartesianDisplaySection";
import type { CartesianViz } from "@/api/charts";

/**
 * A BAR CHART SWITCHED TO LINE USED TO STAY ON ITS SIDE, WITH NO CONTROL TO STAND IT BACK UP.
 *
 * `cartesian.horizontal` lives in the SHARED cartesian settings namespace (`cartesian-codec.ts`), so
 * it survives a chart-type switch — `cartesianToSettings({horizontal: true})` writes
 * `cartesian.horizontal` and `cartesianFromParams` reads it back on whatever kind the chart is now.
 * `buildCartesianOption` honoured it on line, area and combo (measured: `xAxis.type` went
 * `category` → `value` and `yAxis.type` `value` → `category` on all three), while
 * `OrientationOption` renders the Horizontal switch only when `kind === "bar"`. Turn a bar chart
 * horizontal, switch it to a line chart, and you have a sideways line chart and nowhere to say so.
 *
 * WHICH HALF TO MOVE — and the answer comes from Metabase, read rather than assumed.
 *
 * **Metabase has no orientation setting at all.** A horizontal bar chart is a SEPARATE display type
 * there, `row`, registered as a peer of `bar` (`visualizations/register.js`) and rendered by a
 * hand-written d3/SVG component (`shared/components/RowChart`) that does not go through the ECharts
 * cartesian pipeline at all. `RowViz` does not call `getCartesianChartDefinition`. There is no
 * `graph.orientation`, no `isHorizontal`, no `transpose`: `buildAxes`
 * (`echarts/cartesian/option/axis.ts:453-475`) is hardcoded `xAxis: buildDimensionAxis(...)`,
 * `yAxis: buildMetricsAxes(...)` with no orientation parameter and no branch. Grepping its whole
 * `echarts/` tree for `=== "row"` returns nothing. **A line, area or combo chart cannot be
 * horizontal in Metabase — the capability does not exist.** The single acknowledgement of row's
 * orientation anywhere in the settings layer is a SUPPRESSION: `lib/settings/series.ts:207` hides
 * the per-series left/right axis picker for `display === "row"`.
 *
 * So the rule copied is "orientation belongs to the bar family", and the honest place for it is the
 * BUILDER — where `isHorizontal` already made exactly this kind of exclusion for the scatter, whose
 * own two value axes have no orientation to swap. Narrowing `!!cfg.horizontal && cfg.kind !==
 * "scatter"` to `!!cfg.horizontal && cfg.kind === "bar"` is that same clause, said about the kinds
 * it turns out to be true of, rather than a new mechanism.
 *
 * IT IS ALSO THIS FILE'S ESTABLISHED SHAPE FOR A SHARED-NAMESPACE KEY. `xMetric` sits in the same
 * namespace and `buildCartesianOption` opens with `if (cfg0.kind === "scatter" && cfg0.xMetric)` —
 * stored on any cartesian chart, honoured on one kind. A user who stored `horizontal: true` and
 * switched to a line chart gets a normal vertical line chart, and switching back to a bar restores
 * the orientation, because the value is REMEMBERED rather than destroyed. Nothing migrates and no
 * saved chart loses a setting.
 *
 * WHAT WAS REJECTED: offering the toggle on line/area/combo. That ships a horizontal line chart, a
 * capability the reference product does not have, on the strength of the builder having happened to
 * honour a flag it was never offered. The research above is a record of absence, not of a considered
 * upstream judgement — but "copy the rule, not the value" cuts toward the narrow reading when the
 * alternative is inventing a chart type in a defect fix.
 */

const cols = [{ name: "cat", type: "varchar" }, { name: "rev", type: "double" }];
const rows: unknown[][] = [["a", 10], ["b", 20], ["c", 30]];

interface Axis { type?: string }
const axesOf = (cfg: CartesianBuildConfig) => {
  const o = buildCartesianOption(cols, rows, cfg) as unknown as { xAxis: Axis | Axis[]; yAxis: Axis | Axis[] };
  const t = (a: Axis | Axis[]) => (Array.isArray(a) ? a[0]?.type : a?.type);
  return { x: t(o.xAxis), y: t(o.yAxis) };
};

describe("the builder honours `horizontal` on a bar chart and nowhere else", () => {
  it("swaps the axes on a bar", () => {
    expect(axesOf({ kind: "bar", xAxis: "cat", yMetrics: ["rev"] })).toEqual({ x: "category", y: "value" });
    expect(axesOf({ kind: "bar", xAxis: "cat", yMetrics: ["rev"], horizontal: true }))
      .toEqual({ x: "value", y: "category" });
  });

  // A row chart IS the horizontal one, so it needs no flag and offers no switch.
  it("a row chart is horizontal with nothing stored", () => {
    expect(axesOf({ kind: "row", xAxis: "cat", yMetrics: ["rev"] })).toEqual({ x: "value", y: "category" });
  });

  it.each(["line", "area", "combo", "scatter"] as const)(
    "leaves a %s upright whatever the stored flag says",
    (kind) => {
      const upright = axesOf({ kind, xAxis: "cat", yMetrics: ["rev"] });
      expect(upright).toEqual({ x: "category", y: "value" });
      expect(axesOf({ kind, xAxis: "cat", yMetrics: ["rev"], horizontal: true })).toEqual(upright);
    },
  );

  // The flag is REMEMBERED, not destroyed: nothing here rewrites the config, so the same object
  // handed back to a bar draws sideways again.
  it("a value ignored on a line is honoured again on a bar", () => {
    const stored = { xAxis: "cat", yMetrics: ["rev"], horizontal: true };
    expect(axesOf({ ...stored, kind: "line" })).toEqual({ x: "category", y: "value" });
    expect(axesOf({ ...stored, kind: "bar" })).toEqual({ x: "value", y: "category" });
  });

  it("isHorizontal says the same thing the axes do", () => {
    expect(isHorizontal({ kind: "bar", horizontal: true })).toBe(true);
    expect(isHorizontal({ kind: "row" })).toBe(true);
    expect(isHorizontal({ kind: "line", horizontal: true })).toBe(false);
    expect(isHorizontal({ kind: "area", horizontal: true })).toBe(false);
    expect(isHorizontal({ kind: "combo", horizontal: true })).toBe(false);
    expect(isHorizontal({ kind: "scatter", horizontal: true })).toBe(false);
  });
});

/**
 * THE PANEL'S HALF: the axis LETTERS the Axes tab prints must follow the same answer. They are
 * relabelled for a horizontal chart — the dimension runs down the left, so its rows are headed "Y"
 * — and that relabelling read `config.horizontal` directly. With the builder narrowed and the panel
 * not, a line chart carrying a stale flag would print "Y label" over the control that titles its
 * horizontal axis: the exact stored-says-one-thing-screen-says-another defect this tab has had
 * removed from it twice already.
 */
describe("the Axes tab labels follow the same rule", () => {
  const withFlag = (kind: CartesianViz) => {
    cleanup();
    render(
      <CartesianDisplaySection kind={kind} config={{ yMetrics: ["rev"], xAxis: "cat", horizontal: true }}
        onChange={() => {}} />,
    );
  };

  it("a horizontal bar heads the dimension rows Y and the measure rows X", () => {
    withFlag("bar");
    expect(screen.getByLabelText("Y label")).toBeInTheDocument();
    expect(screen.getByLabelText("X scale")).toBeInTheDocument();
  });

  it.each(["line", "area", "combo"] as const)(
    "a %s carrying a stale flag is labelled upright",
    (kind) => {
      withFlag(kind);
      expect(screen.getByLabelText("X label")).toBeInTheDocument();
      expect(screen.getByLabelText("Y scale")).toBeInTheDocument();
    },
  );

  // …and the X-axis-scale select comes back with it. That row is withheld from a horizontal chart
  // because `usesXScale` is false there, which is now false only for a bar.
  it("a stale flag no longer withholds the X axis scale select from a line", () => {
    withFlag("line");
    expect(screen.getByLabelText("X axis scale")).toBeInTheDocument();
    withFlag("bar");
    expect(screen.queryByLabelText("X axis scale")).not.toBeInTheDocument();
  });
});

/**
 * …AND THE TAB'S TWO GROUP HEADINGS, which are a THIRD site that answers the same question.
 *
 * A surviving mutant is why this block exists. `CartesianAxesTab` computes its own `horiz` to head
 * the two `AxisGroup`s, and reverting only that one to `kind === "row" || !!config.horizontal` broke
 * nothing: `CartesianDisplaySection` — which every other assertion above renders — does not use the
 * tab at all, it composes `AxesOptions` and `AxisRangeOptions` directly. The studio's Axes tab
 * (`viz/cartesian.tsx`, the `axes` control) is the only caller of `CartesianAxesTab`, so the
 * headings a real user reads were the one copy of the rule under no test.
 *
 * Three identical expressions in one file is exactly the shape that hides a defect behind its
 * neighbours; each has its own mutant now, and this is the one that needed a new assertion.
 */
describe("the Axes tab's group headings follow the same rule", () => {
  const headings = (kind: CartesianViz) => {
    cleanup();
    render(
      <CartesianAxesTab kind={kind} config={{ yMetrics: ["rev"], xAxis: "cat", horizontal: true }}
        onChange={() => {}} />,
    );
    return screen.getAllByRole("group").map((el) => el.getAttribute("aria-label"));
  };

  // The dimension group leads, so a horizontal chart heads it "Y-axis" — its categories run down
  // the left.
  it("a horizontal bar heads the dimension group Y-axis", () => {
    expect(headings("bar")).toEqual(["Y-axis", "X-axis"]);
  });

  it("a row chart does too, with nothing stored", () => {
    cleanup();
    render(<CartesianAxesTab kind="row" config={{ yMetrics: ["rev"], xAxis: "cat" }} onChange={() => {}} />);
    expect(screen.getAllByRole("group").map((el) => el.getAttribute("aria-label")))
      .toEqual(["Y-axis", "X-axis"]);
  });

  it.each(["line", "area", "combo"] as const)("a %s carrying a stale flag is headed upright", (kind) => {
    expect(headings(kind)).toEqual(["X-axis", "Y-axis"]);
  });
});
