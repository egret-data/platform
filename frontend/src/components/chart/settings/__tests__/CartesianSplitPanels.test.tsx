import { describe, it, expect, vi } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CartesianDisplaySection } from "../CartesianDisplaySection";
import type { CartesianConfig, CartesianViz } from "@/api/charts";

/**
 * THE PANEL SIDE OF SPLIT PANELS — where "Stack series" is offered, and the three controls that go
 * away while it is on.
 *
 * Metabase enforces every one of these mutual exclusions TWICE: the control is hidden (`getHidden`
 * returns true under `graph.split_panels`) AND the stored value is overridden (`getDefault` returns
 * null for `stackable.stack_type`, "left" for `series.axis`). A recon that wrote the settings
 * straight into `visualization_settings` saw only the override and reported these as silent
 * overrides rather than hidden controls; `lib/settings/graph.ts:340-370`, `:888` and
 * `lib/settings/series.ts:205-212` say both. egret does both too — the builder clears the values
 * (`cartesian.splitPanels.test.ts`) and this file is the other half.
 */
const draw = (config: Partial<CartesianConfig>, kind: CartesianViz = "bar", seriesNames?: string[]) => {
  const onChange = vi.fn();
  render(
    <CartesianDisplaySection
      kind={kind}
      config={{ xAxis: "x", yMetrics: ["rev", "cost"], ...config } as CartesianConfig}
      onChange={onChange}
      seriesNames={seriesNames}
    />,
  );
  return onChange;
};

describe("the Stack series toggle", () => {
  it("is offered on a chart with two series", () => {
    draw({});
    expect(screen.getByLabelText("Stack series")).toBeInTheDocument();
  });

  it("is withheld from a chart with one", () => {
    // Metabase's `SPLIT_PANELS_SETTINGS.getHidden`: `visibleDisplays.length <= 1`. One series has
    // nothing to be separated from, and its own panel is the chart it already was.
    draw({ yMetrics: ["rev"] });
    expect(screen.queryByLabelText("Stack series")).not.toBeInTheDocument();
  });

  it("counts BANDS, not metrics, on a broken-out chart", () => {
    // One metric, three bands: three panels. The count is a property of the DATA, which is why it
    // arrives as `seriesNames` rather than being read off the config — the same reason Stacking's
    // own count does.
    draw({ yMetrics: ["rev"], series: "g" }, "bar", ["A", "B", "C"]);
    expect(screen.getByLabelText("Stack series")).toBeInTheDocument();
    cleanup();
    draw({ yMetrics: ["rev"], series: "g" }, "bar", ["A"]);
    expect(screen.queryByLabelText("Stack series")).not.toBeInTheDocument();
  });

  it("is withheld from a row chart and from a metric-against-metric scatter", () => {
    // The two shapes `splitPanelsApplies` refuses, asked here through the builder rather than
    // restated. A row chart already runs its categories down the card; a metric-vs-metric scatter
    // leaves `buildCartesianOption` through `scatterXY` before any of this arithmetic runs.
    draw({ horizontal: true });
    expect(screen.queryByLabelText("Stack series")).not.toBeInTheDocument();
    cleanup();
    draw({ xMetric: "a" }, "scatter");
    expect(screen.queryByLabelText("Stack series")).not.toBeInTheDocument();
    cleanup();
    // …and IS offered on a scatter over a dimension, which does reach the panels.
    draw({}, "scatter");
    expect(screen.getByLabelText("Stack series")).toBeInTheDocument();
  });

  it("stores true and clears back to undefined", async () => {
    const user = userEvent.setup();
    const onChange = draw({});
    await user.click(screen.getByLabelText("Stack series"));
    expect(onChange).toHaveBeenCalledWith({ splitPanels: true });
    cleanup();
    const off = draw({ splitPanels: true });
    await user.click(screen.getByLabelText("Stack series"));
    // `undefined`, not `false`: the codec drops undefined rather than storing it, so turning the
    // feature off leaves no key behind — the same shape every other optional flag here uses.
    expect(off).toHaveBeenCalledWith({ splitPanels: undefined });
  });
});

describe("the three controls Stack series takes away", () => {
  it("hides Stacking", () => {
    draw({ yMetrics: ["rev", "cost"] });
    expect(screen.getByLabelText("Stacking")).toBeInTheDocument();
    cleanup();
    draw({ yMetrics: ["rev", "cost"], splitPanels: true });
    expect(screen.queryByLabelText("Stacking")).not.toBeInTheDocument();
  });

  it("hides the per-series Y-axis side selects", () => {
    draw({ yMetrics: ["rev", "cost"] });
    expect(screen.getByLabelText("axis rev")).toBeInTheDocument();
    cleanup();
    draw({ yMetrics: ["rev", "cost"], splitPanels: true });
    expect(screen.queryByLabelText("axis rev")).not.toBeInTheDocument();
  });

  it("hides Split y-axis when necessary", () => {
    draw({ yMetrics: ["rev", "cost"] });
    expect(screen.getByLabelText("Split y-axis when necessary")).toBeInTheDocument();
    cleanup();
    draw({ yMetrics: ["rev", "cost"], splitPanels: true });
    expect(screen.queryByLabelText("Split y-axis when necessary")).not.toBeInTheDocument();
  });

  it("gives all three back when a STALE flag sits on a chart that cannot take panels", () => {
    // `splitPanels` survives on a config the way `horizontal` survives on a line chart — nothing is
    // erased when the kind changes. A row chart carrying the flag still draws one shared value axis,
    // so withdrawing its axis controls would take away settings that are live.
    draw({ yMetrics: ["rev", "cost"], splitPanels: true, horizontal: true });
    expect(screen.getByLabelText("Stacking")).toBeInTheDocument();
    expect(screen.getByLabelText("Split y-axis when necessary")).toBeInTheDocument();
    cleanup();
    // …and the Y-axis SIDE selects with them, which the row chart above cannot show either way (a
    // horizontal chart has one value axis and no second edge). A metric-against-metric scatter is
    // the case that separates the two gates: it refuses panels while still drawing two sides.
    draw({ yMetrics: ["rev", "cost"], xMetric: "a", splitPanels: true }, "scatter");
    expect(screen.getByLabelText("axis rev")).toBeInTheDocument();
  });
});
