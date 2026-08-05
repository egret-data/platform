import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TrendComparisonsSection } from "../TrendComparisonsSection";
import type { TrendComparison } from "@/components/chart/trend-comparison";

/**
 * The trend's Comparisons editor.
 *
 * The panel's SHAPE is Metabase's, driven at :3033 rather than read off a constant: a "Comparisons"
 * heading, one row per comparison carrying a type dropdown and a remove ✕, an "Add comparison"
 * action beneath, and that action DISABLED once three rows exist. What is asserted here is what a
 * user can reach and what is stored; whether it paints is `SmartScalarChart.test.tsx`'s job and the
 * live check's.
 */
function Stateful({ initial, unit }: { initial?: TrendComparison[]; unit?: string }) {
  const [cmp, setCmp] = useState<TrendComparison[] | undefined>(initial);
  return <TrendComparisonsSection comparisons={cmp} unit={unit} onChange={setCmp} />;
}

describe("TrendComparisonsSection", () => {
  it("offers all four types on a chart grouped by a time period", async () => {
    const user = userEvent.setup();
    render(<Stateful initial={[{ id: "1", type: "previousPeriod" }]} unit="month" />);
    await user.click(screen.getByLabelText("Comparison 1 type"));
    for (const name of ["Previous month", "months ago", "Previous value", "Custom value…"]) {
      expect(await screen.findByRole("option", { name })).toBeInTheDocument();
    }
  });

  /**
   * THE GATE, and it is the reason this control takes `unit` at all. Two of the four types have
   * nothing to count without a period, so they are ABSENT rather than present-and-inert — Metabase
   * omits them the same way (`getComparisonOptions` returns early with no `dateUnit`), and a
   * control offered for a chart that cannot honour it is the defect this programme keeps closing.
   */
  it("withholds the two period types from a chart with no period", async () => {
    const user = userEvent.setup();
    render(<Stateful initial={[{ id: "1", type: "previousValue" }]} />);
    await user.click(screen.getByLabelText("Comparison 1 type"));
    expect(await screen.findByRole("option", { name: "Previous value" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Previous month|periods ago|months ago/ })).toBeNull();
  });

  it("names the period type after the unit the chart is grouped by", async () => {
    const user = userEvent.setup();
    render(<Stateful initial={[{ id: "1", type: "previousPeriod" }]} unit="quarter" />);
    await user.click(screen.getByLabelText("Comparison 1 type"));
    expect(await screen.findByRole("option", { name: "Previous quarter" })).toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "quarters ago" })).toBeInTheDocument();
  });

  it("adds a row, and adds the DEFAULT rather than an untyped one", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<TrendComparisonsSection unit="month" onChange={onChange} />);
    await user.click(screen.getByRole("button", { name: /Add comparison/ }));
    expect(onChange.mock.lastCall?.[0]).toHaveLength(1);
    expect(onChange.mock.lastCall?.[0][0].type).toBe("previousPeriod");
    // On an ungrouped chart the default is the other one — the same branch the card itself takes.
    cleanup();
    render(<TrendComparisonsSection onChange={onChange} />);
    await user.click(screen.getByRole("button", { name: /Add comparison/ }));
    expect(onChange.mock.lastCall?.[0][0].type).toBe("previousValue");
  });

  it("stops at three, which is Metabase's cap", async () => {
    const user = userEvent.setup();
    render(<Stateful initial={[]} unit="month" />);
    const add = () => screen.getByRole("button", { name: /Add comparison/ });
    for (let i = 0; i < 3; i++) {
      expect(add()).not.toBeDisabled();
      await user.click(add());
    }
    expect(screen.getAllByLabelText(/^Comparison \d type$/)).toHaveLength(3);
    expect(add()).toBeDisabled();
  });

  it("removes the row it was asked to remove, not the first one", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <TrendComparisonsSection unit="month" onChange={onChange}
        comparisons={[
          { id: "a", type: "previousPeriod" },
          { id: "b", type: "previousValue" },
        ]} />,
    );
    await user.click(screen.getByLabelText("Remove comparison 2"));
    expect(onChange).toHaveBeenCalledWith([{ id: "a", type: "previousPeriod" }]);
  });

  /**
   * REMOVING THE LAST ROW MUST STORE `undefined`, NOT `[]`.
   *
   * `resolveComparisons` reads both as "fall back to the default", so a stored `[]` is a setting
   * that can never be read back as anything — and it would persist a key on a chart that is now in
   * exactly the state a brand-new one is in. Every other panel here keeps the same "store only what
   * differs from the default" rule.
   */
  it("clears the setting entirely when the last row goes", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <TrendComparisonsSection unit="month" onChange={onChange}
        comparisons={[{ id: "a", type: "previousPeriod" }]} />,
    );
    await user.click(screen.getByLabelText("Remove comparison 1"));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  /**
   * SWITCHING TYPE DROPS THE OTHER TYPE'S FIELDS. A "500" typed as a custom value is not a count of
   * periods; carrying it over would silently change what the row means while the input still looked
   * as the user left it. Asserted with `toStrictEqual` because the ABSENCE of `value` and `label`
   * is the whole point — `toEqual` skips undefined properties and cannot see the difference.
   */
  it("does not carry one type's fields into another", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <TrendComparisonsSection unit="month" onChange={onChange}
        comparisons={[{ id: "a", type: "staticNumber", value: 500, label: "Goal" }]} />,
    );
    await user.click(screen.getByLabelText("Comparison 1 type"));
    await user.click(await screen.findByRole("option", { name: "Previous value" }));
    expect(onChange.mock.lastCall?.[0]).toStrictEqual([
      { id: "a", type: "previousValue", value: undefined, label: undefined },
    ]);
  });

  it("gives a custom value both of the fields its caption needs", () => {
    render(<Stateful initial={[{ id: "a", type: "staticNumber", value: 500, label: "Goal" }]} unit="month" />);
    expect(screen.getByLabelText("Value")).toHaveValue("500");
    expect(screen.getByLabelText("Label")).toHaveValue("Goal");
  });

  it("gives a periods-ago row a count, and floors it to whole periods", () => {
    const onChange = vi.fn();
    render(
      <TrendComparisonsSection unit="month" onChange={onChange}
        comparisons={[{ id: "a", type: "periodsAgo", value: 3 }]} />,
    );
    const box = screen.getByLabelText("How many months back");
    expect(box).toHaveValue("3");
    // A fractional entry is floored on the way out rather than rejected in the box: `DecimalInput`
    // is a TEXT field precisely so a half-typed number is not thrown away mid-keystroke, so the
    // constraint belongs on the value it emits.
    fireEvent.change(box, { target: { value: "2.7" } });
    expect(onChange.mock.lastCall?.[0][0].value).toBe(2);
    // …and never below one, which `isComparisonValid` would drop.
    fireEvent.change(box, { target: { value: "0" } });
    expect(onChange.mock.lastCall?.[0][0].value).toBe(1);
  });

  it("shows the type controls only for the types that have them", () => {
    render(<Stateful initial={[{ id: "a", type: "previousPeriod" }]} unit="month" />);
    // The negative, and the one an unconditional row breaks: "Previous month" is fully described by
    // its own name and must not draw an empty Value/Label pair beneath it.
    expect(screen.queryByLabelText("Value")).toBeNull();
    expect(screen.queryByLabelText(/How many/)).toBeNull();
  });
});
