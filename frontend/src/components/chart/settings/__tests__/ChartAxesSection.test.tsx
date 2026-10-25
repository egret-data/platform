import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChartAxesSection, xTicksChoiceOf, xTicksPatch, type AxesConfig } from "../ChartAxesSection";

/**
 * The Axes tab the waterfall, the box plot and the sankey share — and specifically its x
 * lines/ticks/rotation select, which had no test at all until cartesian's panel started using the
 * same projection. Without one, the whole waterfall/box-plot side of `xTicksChoiceOf`/`xTicksPatch`
 * could be broken by a change made for cartesian's sake and nothing here would say so.
 *
 * Assertions are on the emitted patch, not the DOM: one control writes THREE stored fields, and the
 * failure worth catching is a patch that sets some of the group. `toStrictEqual`, because `toEqual`
 * ignores `undefined` properties and so cannot tell a patch that CLEARS the other keys from one that
 * quietly leaves them behind.
 *
 * THE EXPECTATIONS BELOW GAINED A THIRD KEY when Metabase's fifth choice, "Compact", was added, and
 * that is `toStrictEqual` doing exactly the job the paragraph above claims for it: `xLabelCompact:
 * undefined` is invisible to `toEqual` and to `toHaveBeenCalledWith`, so under either of those the
 * new key could have been added — or forgotten — with every assertion here still green. Two tests
 * failing on a key nobody asserted is the cost of that guarantee and the proof it is real.
 */
const cfg = (over: Partial<AxesConfig> = {}): AxesConfig => ({ ...over });

describe("ChartAxesSection — the x lines/ticks/rotation select", () => {
  it("choosing Rotate 90° stores the rotation and clears the hide", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ChartAxesSection config={cfg({ xAxisTicks: false })} onChange={onChange} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Rotate 90°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 90, xAxisTicks: undefined, xLabelCompact: undefined });
  });

  it("choosing Hide turns the axis off and drops the rotation with it", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ChartAxesSection config={cfg({ xLabelRotation: 45 })} onChange={onChange} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Hide" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: false, xLabelRotation: undefined, xLabelCompact: undefined });
  });

  it("displays what is stored", () => {
    const { rerender } = render(<ChartAxesSection config={cfg({ xLabelRotation: 45 })} onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Rotate 45°");
    rerender(<ChartAxesSection config={cfg({ xLabelRotation: 45, xAxisTicks: false })} onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Hide");
  });

  /** These charts' x is always the ordinal sequence of categories, so — unlike cartesian's, which
   *  disables them on a value x-axis — every choice is live here. */
  it("offers the rotations unconditionally, since these charts have no other kind of x-axis", async () => {
    const user = userEvent.setup();
    render(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    expect(await screen.findByRole("option", { name: "Rotate 45°" })).not.toHaveAttribute("aria-disabled", "true");
  });
});

/**
 * THE TWO CHARTS THAT SHARE THIS TAB GET ONE NEW SETTING EACH, AND NOT THE SAME ONE — which is the
 * whole reason both are props rather than facts about the tab.
 *
 * Both directions on both settings, because a one-sided assertion cannot tell "correctly withheld"
 * from "never wired up": the offering test would pass against a component that always showed the
 * row, and the withholding test would pass against one that never did.
 */
describe("ChartAxesSection — Compact is the waterfall's and Unpin is the box plot's", () => {
  it("offers Compact only where a caller says the chart has a label formatter", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<ChartAxesSection config={cfg()} offerCompact onChange={() => {}} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    expect(await screen.findByRole("option", { name: "Compact" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    // The box plot's arm. Its category axis has no `axisLabel.formatter` at all, so the option is
    // ABSENT rather than greyed — there is no state of that chart in which it would do something.
    rerender(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    expect(await screen.findByRole("option", { name: "Show" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Compact" })).toBeNull();
  });

  it("stores Compact and clears the rotation with it — Metabase's five values are exclusive", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ChartAxesSection config={cfg({ xLabelRotation: 45 })} offerCompact onChange={onChange} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Compact" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({
      xLabelCompact: true, xAxisTicks: undefined, xLabelRotation: undefined,
    });
  });

  it("shows a stored Compact on the trigger, and lets a rotation outrank it", () => {
    const { rerender } = render(<ChartAxesSection config={cfg({ xLabelCompact: true })} offerCompact onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Compact");
    // The pair this control cannot write but an older build's config can hold — see
    // `xTicksChoiceOf`, which reports the more visible of the two rather than inventing a sixth
    // choice for the combination.
    rerender(<ChartAxesSection config={cfg({ xLabelCompact: true, xLabelRotation: 90 })} offerCompact onChange={() => {}} />);
    expect(screen.getByLabelText("X show lines and tick marks")).toHaveTextContent("Rotate 90°");
  });

  it("offers Unpin from zero only where a caller says the display allows it", () => {
    const { rerender } = render(<ChartAxesSection config={cfg()} offerUnpin onChange={() => {}} />);
    expect(screen.getByLabelText("Unpin from zero")).toBeInTheDocument();
    // The waterfall's arm: `isYAxisUnpinFromZeroValid` excludes that display by name.
    rerender(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    expect(screen.queryByLabelText("Unpin from zero")).toBeNull();
  });

  it("withdraws Unpin once a bound is set, and brings it back when the bound is cleared", () => {
    // Metabase's own gate, `!settings["graph.y_axis.auto_range"]`, read off egret's derived pair.
    // The second half is what stops this passing against a row that is simply never rendered.
    const { rerender } = render(<ChartAxesSection config={cfg({ yMin: 0 })} offerUnpin onChange={() => {}} />);
    expect(screen.queryByLabelText("Unpin from zero")).toBeNull();
    rerender(<ChartAxesSection config={cfg({ yMax: 100 })} offerUnpin onChange={() => {}} />);
    expect(screen.queryByLabelText("Unpin from zero")).toBeNull();
    rerender(<ChartAxesSection config={cfg()} offerUnpin onChange={() => {}} />);
    expect(screen.getByLabelText("Unpin from zero")).toBeInTheDocument();
  });

  it("stores Unpin as true, and CLEARS the key rather than storing false", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(<ChartAxesSection config={cfg()} offerUnpin onChange={onChange} />);
    await user.click(screen.getByLabelText("Unpin from zero"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: true });
    // Off is an ABSENCE, not a stored `false`, so a chart that never touched this reads identically
    // to one toggled on and back off — the same rule every other "unset means the default" key in
    // this panel keeps, and what lets `mergeVizSettings` delete it.
    rerender(<ChartAxesSection config={cfg({ unpinFromZero: true })} offerUnpin onChange={onChange} />);
    await user.click(screen.getByLabelText("Unpin from zero"));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ unpinFromZero: undefined });
  });
});

/**
 * Metabase's `getHidden` on `graph.y_axis.split_number` — "Number of tick marks" —
 * (lib/settings/graph.ts): `settings["graph.y_axis.axis_enabled"] === false`. With the value axis'
 * lines and ticks turned off there are no tick marks to count, so the box asks for a number that
 * changes nothing. Same setting, same condition, so it is ported rather than reasoned about.
 *
 * Both directions: a fixture with the axis on cannot show the box hides, and one with it off cannot
 * show it is still offered.
 */
describe("ChartAxesSection — the tick count follows the axis it counts", () => {
  it("hides Number of tick marks once the value axis' lines and ticks are off", () => {
    render(<ChartAxesSection config={cfg({ yAxisTicks: false })} onChange={() => {}} />);
    expect(screen.queryByLabelText("Number of tick marks")).toBeNull();
  });

  it("offers it while they are drawn", () => {
    render(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    expect(screen.getByLabelText("Number of tick marks")).toBeInTheDocument();
  });
});

/** The X and Y settings are each announced under the axis they belong to, so a screen reader (and
 *  `within`) can tell the two halves of this tab apart rather than meeting one flat run of rows. */
describe("ChartAxesSection — each half is a labelled group", () => {
  it("files the X rows under X-axis and the Y rows under Y-axis", () => {
    render(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    const x = screen.getByRole("group", { name: "X-axis" });
    const y = screen.getByRole("group", { name: "Y-axis" });
    expect(within(x).getByLabelText("X show lines and tick marks")).toBeInTheDocument();
    expect(within(x).queryByLabelText("Y scale")).toBeNull();
    expect(within(y).getByLabelText("Y scale")).toBeInTheDocument();
    expect(within(y).queryByLabelText("X label")).toBeNull();
  });
});

/** The projection itself, asserted once away from either panel: it is the thing the two must not
 *  disagree about, and both of them go through exactly these two functions. */
describe("xTicksChoiceOf / xTicksPatch", () => {
  it("round-trips each of the four choices through the pair of fields", () => {
    for (const v of ["hide", "show", "rotate45", "rotate90"] as const) {
      expect(xTicksChoiceOf(xTicksPatch(v))).toBe(v);
    }
  });

  it("reads an untouched config as Show", () => {
    expect(xTicksChoiceOf({})).toBe("show");
  });
});

/** KI-054: the waterfall and box plot name these rows the way every other chart's Axes tab does —
 *  "X label" / "Y label" over each title, "Y scale" over the scale — not "Show label" and "Scale". */
describe("ChartAxesSection — row names match the other charts' Axes tab", () => {
  it("titles each axis' label row with its letter and the scale row with Y", () => {
    render(<ChartAxesSection config={cfg()} onChange={() => {}} />);
    const x = screen.getByRole("group", { name: "X-axis" });
    const y = screen.getByRole("group", { name: "Y-axis" });
    expect(within(x).getByText("X label", { selector: "label" })).toBeInTheDocument();
    expect(within(y).getByText("Y label", { selector: "label" })).toBeInTheDocument();
    expect(within(y).getByText("Y scale", { selector: "label" })).toBeInTheDocument();
    expect(screen.queryByText("Show label")).toBeNull();
    expect(screen.queryByText("Scale", { selector: "label" })).toBeNull();
  });
});
