// src/components/chart/__tests__/PieBreakdown.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { PieBreakdown } from "../PieBreakdown";
import type { PieBreakdownModel } from "@/components/chart/echarts/options/pie";

const model: PieBreakdownModel = {
  levels: ["region", "product", "channel"],
  metric: "Revenue",
  notes: [],
  rows: [
    { depth: 0, name: "R1", parents: [], value: "1,020", share: "50.0%" },
    { depth: 1, name: "P1", parents: ["R1"], value: "1,000", share: "98.0%" },
    { depth: 2, name: "x", parents: ["R1", "P1"], value: "1,000", share: "100.0%" },
    { depth: 1, name: "Other", parents: ["R1"], value: "20", share: "2.0%" },
  ],
};

const renderBreakdown = (m: Partial<PieBreakdownModel> = {}, open = true) =>
  render(<PieBreakdown breakdown={{ ...model, ...m }} defaultOpen={open} />);

describe("PieBreakdown", () => {
  // The point of the whole thing: a real table, reachable by role, not a pile of styled divs that
  // happen to line up. Asserted through the accessibility tree — which is the only thing a screen
  // reader can see — rather than through any class name.
  it("is a real table an assistive technology can navigate", () => {
    renderBreakdown();
    const table = screen.getByRole("table");
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "Ring", "Slice", "Within", "Revenue", "Share of parent",
    ]);
    // Header row + one row per slice.
    expect(within(table).getAllByRole("row")).toHaveLength(5);
    // The table says what it is, for a reader who lands on it out of context.
    expect(table).toHaveAccessibleName(/share of the slice it sits inside/i);
  });

  it("gives every slice a row naming its ring, its ancestors, its value and its share", () => {
    renderBreakdown();
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell").map((c) => c.textContent))).toEqual([
      ["region", "R1", "Whole chart", "1,020", "50.0%"],
      ["product", "P1", "R1", "1,000", "98.0%"],
      ["channel", "x", "R1 › P1", "1,000", "100.0%"],
      ["product", "Other", "R1", "20", "2.0%"],
    ]);
  });

  // Depth is carried by the `Ring` and `Within` cells, which a screen reader reads with their
  // column headers; the indent is the sighted reader's version of the same fact, and nothing else
  // depends on it.
  it("indents a nested slice's name without making the indent the only cue", () => {
    renderBreakdown();
    // Rows after the header: R1 (ring 0), P1 (ring 1), x (ring 2), Other (ring 1).
    const indentAt = (row: number) =>
      within(screen.getAllByRole("row")[row]).getAllByRole("cell")[1].getAttribute("style");
    expect(indentAt(1)).not.toBe(indentAt(2));
    expect(indentAt(2)).not.toBe(indentAt(3));
    // ...and two slices at the same depth are indented identically, whoever their parent is.
    expect(indentAt(2)).toBe(indentAt(4));
  });

  // It is a disclosure, not a permanently open block: the table is a long list beside a chart that
  // is meant to be looked at, and every reader — not only a screen-reader one — can open it.
  it("is a disclosure that a sighted keyboard user can find and open", () => {
    const { container } = render(<PieBreakdown breakdown={model} />);
    const details = container.querySelector("details")!;
    expect(details).not.toHaveAttribute("open");
    const summary = within(details).getByText(/Slice breakdown/);
    expect(summary.tagName).toBe("SUMMARY");
    // It says what opening it gets you, rather than only "details".
    expect(summary).toHaveTextContent("4 slices");
    expect(summary).toHaveTextContent("3 rings");
  });

  it("states a ring-total divergence before the numbers it qualifies", () => {
    renderBreakdown({ notes: ["Note: the product ring totals 30, not 20 like the chart's overall total."] });
    const note = screen.getByText(/the product ring totals 30/);
    expect(note.compareDocumentPosition(screen.getByRole("table")) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
  });

  // The decision this pins: below two rings there is nothing the arcs encode that the chart's own
  // aria description does not already read out, and a second copy of the same flat list is a cost
  // to a screen-reader user, not a gain. `buildPieBreakdown` returns no rows there, and this
  // renders nothing at all rather than an empty disclosure.
  it("renders nothing when there is no breakdown to give", () => {
    const { container } = render(
      <PieBreakdown breakdown={{ rows: [], levels: [], metric: "m", notes: [] }} />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
