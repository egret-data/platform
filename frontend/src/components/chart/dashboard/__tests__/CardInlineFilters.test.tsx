import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { CardInlineFilters } from "../CardInlineFilters";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "p2", name: "City", slug: "city", type: "category", operator: "is" },
];

const base = () => ({
  parameters: params,
  values: {} as Record<string, unknown>,
  optionsBySlug: { region: ["EU", "US"], city: ["Hanoi"] },
  onChange: vi.fn(),
  collapsed: false,
  listRef: { current: null },
});

describe("CardInlineFilters expanded", () => {
  it("draws one pill per hosted filter and reports the RAW value the pill emits", () => {
    // The raw/display trap: asserting the label "EU" would pass against an implementation
    // storing "EU", which is not what a MULTI-value category holds (multiSelect.ts:56-64
    // defaults category to multi).
    const b = base();
    render(<CardInlineFilters {...b} />);
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    fireEvent.click(screen.getByLabelText("option EU"));
    expect(b.onChange).toHaveBeenCalledWith("region", ["EU"]);
  });

  it("offers no Show filters trigger while there is room", () => {
    render(<CardInlineFilters {...base()} />);
    expect(screen.queryByRole("button", { name: "Show filters" })).toBeNull();
  });

  it("renders nothing at all for a card that hosts no filters", () => {
    const { container } = render(<CardInlineFilters {...base()} parameters={[]} />);
    expect(container.firstChild).toBeNull();
  });
});

describe("CardInlineFilters collapsed", () => {
  it("replaces the list with a Show filters trigger", () => {
    render(<CardInlineFilters {...base()} collapsed />);
    expect(screen.getByRole("button", { name: "Show filters" })).toBeTruthy();
    // Nothing but the trigger is REACHABLE: the pills live inside a closed popover, and the
    // measurement copy is aria-hidden and therefore out of the accessibility tree.
    expect(screen.queryAllByRole("button", { name: "Region" })).toHaveLength(0);
  });

  it("counts only the filters that have a value, as Metabase's badge does", () => {
    // CollapsibleDashboardParameterList.tsx:29-31 — `parameters.filter((p) => p.value != null)`.
    render(<CardInlineFilters {...base()} collapsed values={{ region: ["EU"], city: [] }} />);
    expect(screen.getByTestId("inline-filter-count").textContent).toBe("1");
  });

  it("shows no count when nothing is set", () => {
    render(<CardInlineFilters {...base()} collapsed />);
    expect(screen.queryByTestId("inline-filter-count")).toBeNull();
  });

  it("opens the filters in a popover", async () => {
    render(<CardInlineFilters {...base()} collapsed />);
    fireEvent.click(screen.getByRole("button", { name: "Show filters" }));
    expect(await screen.findByRole("button", { name: "City" })).toBeTruthy();
  });
});

describe("CardInlineFilters measurement copy", () => {
  // THE ONE THING JSDOM CAN SAY ABOUT THE OSCILLATION FIX. jsdom reports 0 for every width, so it
  // cannot show the flicker — but it CAN pin the two structural properties that prevent it: the
  // always-expanded copy is present in BOTH states (so a collapsed list still has a width to be
  // un-collapsed by), and it is aria-hidden (so it never doubles the pills a reader can reach).
  // The flicker itself is measured in the browser; see the PR body.
  it("renders an always-expanded copy CARRYING THE PILLS, whether or not the list is collapsed", () => {
    // "Present" is not enough — an empty copy measures 0 and produces exactly the oscillation
    // this exists to stop: collapsed → copy is empty → "there is room" → expand → collapse → …
    // So the assertion is on the copy's CONTENT, in both states.
    const expected = ["Region", "City"];
    const { unmount } = render(<CardInlineFilters {...base()} />);
    const expanded = within(screen.getByTestId("inline-filter-measure"));
    expect(expected.map((n) => !!expanded.getByLabelText(n))).toEqual([true, true]);
    unmount();
    render(<CardInlineFilters {...base()} collapsed />);
    const collapsed = within(screen.getByTestId("inline-filter-measure"));
    expect(expected.map((n) => !!collapsed.getByLabelText(n))).toEqual([true, true]);
  });

  it("keeps the copy out of the accessibility tree", () => {
    render(<CardInlineFilters {...base()} />);
    expect(screen.getByTestId("inline-filter-measure").getAttribute("aria-hidden")).toBe("true");
    // One reachable Region pill, not two — the proof that the copy is not counted twice.
    expect(screen.queryAllByRole("button", { name: "Region" })).toHaveLength(1);
  });
});

describe("CardInlineFilters in edit mode", () => {
  it("selects the filter instead of picking a value, and marks the active one", () => {
    // This is the ONLY route from a card-hosted filter back to its settings panel, and so the
    // only route back to the top bar. If it stops selecting, the filter is unreachable.
    const b = base();
    const onSelect = vi.fn();
    render(<CardInlineFilters {...b} onSelect={onSelect} activeId="p2" />);
    expect(screen.getByRole("button", { name: "City" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    expect(onSelect).toHaveBeenCalledWith("p1");
    expect(b.onChange).not.toHaveBeenCalled();
  });
});
