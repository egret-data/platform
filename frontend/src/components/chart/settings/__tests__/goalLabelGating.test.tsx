import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { GoalOptions } from "../CartesianDisplaySection";
import { BoxplotDisplaySection } from "../BoxplotDisplaySection";
import { WaterfallFormatSection } from "../WaterfallDisplaySection";

/**
 * A GOAL LABEL WITH NO GOAL TO LABEL IS AN INERT CONTROL, AND THAT IS THE WHOLE OF THIS CHANGE.
 *
 * WHAT THE SWEEP ASKED FOR AND WHY IT IS NOT WHAT WAS DONE. Metabase has a `graph.show_goal` toggle
 * (`lib/settings/goal.ts`, default false) which gates both Goal value and Goal label through
 * `getHidden: vizSettings["graph.show_goal"] !== true`, and the suggestion was that egret grow the
 * same boolean. It has not, and the reason is that Metabase's boolean exists to solve a problem
 * egret does not have: `graph.goal_value` is declared `widget: "number"` with `getDefault: () => 0`,
 * so it ALWAYS holds a number and cannot express "no goal" by itself — the boolean is what carries
 * that state. egret's `goalValue` is `number | undefined` behind a `DecimalInput` whose placeholder
 * is literally "none", and all three builders already gate the drawn line on `cfg.goalValue != null`
 * (`echarts/options/cartesian.ts`, `waterfall.ts`, `boxplot.ts`). "Empty" therefore already means
 * "off", honestly, everywhere it is read.
 *
 * Adding the boolean anyway would buy nothing and cost something real: a second source of truth for
 * one state, in a config the codec round-trips (`viz/cartesian-codec.ts`'s `KEYS`) plus three
 * builder interfaces, with a `{ goalValue: 50, showGoal: false }` combination reachable from stored
 * JSON and no obvious answer for what it should draw. This programme has repeatedly found egret
 * deliberately ahead of the reference and the rule has been to say so rather than to converge; #424
 * is the most recent.
 *
 * WHAT WAS REAL IS THE SECOND CONTROL. With no goal set, egret drew a Goal LABEL box whose value
 * nothing would read — `goalLabel` is consulted only inside the `goalValue != null` branch in all
 * three builders — which is exactly the "a control that saves a value and paints nothing" defect
 * this codebase has closed five or six times now (`view_as` off the table, `show_mini_bar` off the
 * pivot, the date separator under an unslashed style, the y-axis split on a breakout). So the label
 * is gated on the goal value, which is egret's own honest "off" signal, and the two panels end up
 * showing the same number of controls as Metabase's for the same state.
 *
 * MEASURED FIRST, in the browser on a bar chart over `MB By Category`: both boxes were drawn under
 * "More options" with nothing stored. Note the sweep's report that egret "shows both boxes always"
 * is not quite right either — the pair already sits behind a collapsed "More options (2)"
 * disclosure on the cartesian panel, so the divergence was narrower than it read.
 *
 * All three panels are asserted, not just the cartesian the sweep named: the box plot and the
 * waterfall carry a byte-identical copy of the same two controls, and a fix to one of three is how
 * this rule would come back.
 */

const PANELS = [
  {
    name: "cartesian",
    render: (config: Record<string, unknown>, onChange = vi.fn()) => {
      render(<GoalOptions kind="bar" config={{ yMetrics: ["rev"], ...config }} onChange={onChange} />);
      return onChange;
    },
  },
  {
    name: "box plot",
    render: (config: Record<string, unknown>, onChange = vi.fn()) => {
      render(<BoxplotDisplaySection config={{ category: "c", value: "v", ...config }} onChange={onChange} />);
      return onChange;
    },
  },
  {
    name: "waterfall",
    render: (config: Record<string, unknown>, onChange = vi.fn()) => {
      render(<WaterfallFormatSection config={{ xAxis: "x", metric: "m", ...config }} onChange={onChange} />);
      return onChange;
    },
  },
] as const;

describe.each(PANELS)("$name — Goal label follows the goal value", ({ render: renderPanel }) => {
  it("offers Goal value whether or not a goal is set — it is the way to set one", () => {
    renderPanel({});
    expect(screen.getByLabelText("Goal value")).toBeInTheDocument();
  });

  it("does not draw a Goal label box when there is no goal to label", () => {
    renderPanel({});
    expect(screen.queryByLabelText("Goal label")).not.toBeInTheDocument();
  });

  it("draws it once a goal value is set", () => {
    renderPanel({ goalValue: 50 });
    expect(screen.getByLabelText("Goal label")).toBeInTheDocument();
  });

  // Zero is a legal goal — a break-even line — and is exactly the value a `!config.goalValue`
  // truthiness test would treat as "off". The builders gate on `!= null` and so must this.
  it("treats a goal of 0 as a goal, not as an empty box", () => {
    renderPanel({ goalValue: 0 });
    expect(screen.getByLabelText("Goal label")).toBeInTheDocument();
  });

  it("still writes the label through when it is shown", () => {
    const onChange = renderPanel({ goalValue: 50 });
    fireEvent.change(screen.getByLabelText("Goal label"), { target: { value: "Target" } });
    expect(onChange).toHaveBeenCalledWith({ goalLabel: "Target" });
  });

  /**
   * A LABEL STORED UNDER AN OLDER CHART, WHOSE GOAL WAS LATER CLEARED, MUST NOT KEEP THE BOX ALIVE.
   *
   * The same rule and the same reason as `FormatSection`'s date separator, which stays hidden under
   * an unslashed style even when a separator is already stored: the control is hidden because
   * nothing reads the value, and a stored value is not a reason for anything to start reading it.
   * The stored `goalLabel` is left alone rather than cleared — it costs a key, it becomes meaningful
   * again the moment a goal is typed, and clearing it would be a mutation on load.
   */
  it("stays hidden when a label is stored but the goal is not", () => {
    renderPanel({ goalLabel: "Target" });
    expect(screen.queryByLabelText("Goal label")).not.toBeInTheDocument();
  });
});
