import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChartAxesSection, xTicksChoiceOf, xTicksPatch, type AxesConfig } from "../ChartAxesSection";

/**
 * The Axes tab the waterfall, the box plot and the sankey share — and specifically its x
 * lines/ticks/rotation select, which had no test at all until cartesian's panel started using the
 * same projection. Without one, the whole waterfall/box-plot side of `xTicksChoiceOf`/`xTicksPatch`
 * could be broken by a change made for cartesian's sake and nothing here would say so.
 *
 * Assertions are on the emitted patch, not the DOM: one control writes TWO stored fields, and the
 * failure worth catching is a patch that sets half the pair. `toStrictEqual`, because `toEqual`
 * ignores `undefined` properties and so cannot tell a patch that CLEARS the other key from one that
 * quietly leaves it behind.
 */
const cfg = (over: Partial<AxesConfig> = {}): AxesConfig => ({ ...over });

describe("ChartAxesSection — the x lines/ticks/rotation select", () => {
  it("choosing Rotate 90° stores the rotation and clears the hide", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ChartAxesSection config={cfg({ xAxisTicks: false })} onChange={onChange} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Rotate 90°" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xLabelRotation: 90, xAxisTicks: undefined });
  });

  it("choosing Hide turns the axis off and drops the rotation with it", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ChartAxesSection config={cfg({ xLabelRotation: 45 })} onChange={onChange} />);
    await user.click(screen.getByLabelText("X show lines and tick marks"));
    await user.click(await screen.findByRole("option", { name: "Hide" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual({ xAxisTicks: false, xLabelRotation: undefined });
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
