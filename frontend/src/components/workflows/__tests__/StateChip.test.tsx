import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { flowActivity, StateChip } from "@/components/workflows/StateChip";

/**
 * The spec's table (§8.1), written out as data — NOT imported from the
 * component, and not derived from a group map, so a state moved to the wrong
 * group in `StateChip.tsx` fails here instead of moving the expectation with it.
 *
 * The token is asserted as the chip's background class: the ink is
 * `text-foreground` on every tone (see `StateChip.tsx` for why), so the
 * background is the only place the token appears.
 */
const EXECUTION: [state: string, name: string, bg: string][] = [
  ["CREATED", "Created", "bg-muted"],
  ["SUBMITTED", "Submitted", "bg-muted"],
  ["QUEUED", "Queued", "bg-muted"],
  ["RESTARTED", "Restarted", "bg-muted"],
  ["RESUBMITTED", "Resubmitted", "bg-muted"],
  ["RUNNING", "Running", "bg-primary/15"],
  ["RETRYING", "Retrying", "bg-primary/15"],
  ["KILLING", "Killing", "bg-primary/15"],
  ["SUCCESS", "Success", "bg-success/15"],
  ["WARNING", "Warning", "bg-highlight/15"],
  ["PAUSED", "Paused", "bg-highlight/15"],
  ["BREAKPOINT", "Breakpoint", "bg-highlight/15"],
  ["FAILED", "Failed", "bg-destructive/15"],
  ["KILLED", "Killed", "bg-destructive/15"],
  ["CANCELLED", "Cancelled", "bg-muted-foreground/15"],
  ["SKIPPED", "Skipped", "bg-muted-foreground/15"],
  ["RETRIED", "Retried", "bg-muted-foreground/15"],
];

/**
 * The user's ruling (2026-10-10): the flow chip says only whether the flow
 * runs. Every execution runs the saved draft, so `draft` and `live` are both
 * Active; the version number is shown elsewhere.
 */
const FLOW: [state: string, name: string, bg: string][] = [
  ["draft", "Active", "bg-success/15"],
  ["live", "Active", "bg-success/15"],
  ["disabled", "Disabled", "bg-muted-foreground/15"],
];

/** The chip is found by its visible word, then checked for the rest. */
function chip(name: string): HTMLElement {
  return screen.getByText(name).closest("[data-state]") as HTMLElement;
}

describe("StateChip", () => {
  it("covers all seventeen execution states", () => {
    expect(EXECUTION).toHaveLength(17);
  });

  it.each(EXECUTION)("execution %s: its name, an aria-hidden icon, and its token", (state, name, bg) => {
    render(<StateChip kind="execution" state={state} />);
    const el = chip(name);
    expect(el).toHaveAttribute("data-state", state);
    const svg = el.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(el.className.split(/\s+/)).toContain(bg);
    expect(el).toHaveClass("text-foreground");
  });

  it.each(FLOW)("flow %s: its name, an aria-hidden icon, and its token", (state, name, bg) => {
    render(<StateChip kind="flow" state={state} />);
    const el = chip(name);
    expect(el).toHaveAttribute("data-state", state);
    expect(el).toHaveTextContent(new RegExp(`^${name}$`));
    const svg = el.querySelector("svg");
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(el.className.split(/\s+/)).toContain(bg);
  });

  it("is named by its word alone, so the icon adds nothing a screen reader hears", () => {
    render(<StateChip kind="execution" state="FAILED" />);
    expect(chip("Failed")).toHaveTextContent(/^Failed$/);
  });

  it("shows a state it does not know as written, on the neutral tone, rather than hiding it", () => {
    render(<StateChip kind="execution" state="SOMETHING_NEW" />);
    const el = chip("SOMETHING_NEW");
    expect(el.className.split(/\s+/)).toContain("bg-muted-foreground/15");
    expect(el.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("does not answer a state string from the object prototype", () => {
    render(<StateChip kind="execution" state="constructor" />);
    expect(chip("constructor").className.split(/\s+/)).toContain("bg-muted-foreground/15");
  });

  it("does not read an execution state as a flow state, or the reverse", () => {
    // `SUCCESS` is not a flow state and `live` is not an execution state: each
    // falls to the unknown branch instead of borrowing the other kind's tone.
    render(
      <>
        <StateChip kind="flow" state="SUCCESS" />
        <StateChip kind="execution" state="live" />
      </>,
    );
    expect(chip("SUCCESS").className.split(/\s+/)).toContain("bg-muted-foreground/15");
    expect(chip("live").className.split(/\s+/)).toContain("bg-muted-foreground/15");
  });

  it("never names a flow by the word Live or Draft, which read as which version runs", () => {
    render(
      <>
        <StateChip kind="flow" state="draft" />
        <StateChip kind="flow" state="live" />
      </>,
    );
    expect(screen.queryByText(/Live|Draft/)).not.toBeInTheDocument();
    expect(screen.getAllByText("Active")).toHaveLength(2);
  });
});

describe("flowActivity", () => {
  it("is active for draft and live, disabled for disabled, and unknown otherwise", () => {
    expect(flowActivity("draft")).toBe("active");
    expect(flowActivity("live")).toBe("active");
    expect(flowActivity("disabled")).toBe("disabled");
    expect(flowActivity("archived")).toBeUndefined();
  });
});
