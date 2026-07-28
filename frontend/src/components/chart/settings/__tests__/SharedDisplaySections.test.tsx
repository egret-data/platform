import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { SharedDisplaySections } from "../SharedDisplaySections";
import type { VizType } from "@/api/charts";

// Renders with no result columns yet and returns the empty-state paragraph's text. Every prop
// besides vizType is a no-op stand-in since the empty-state branch returns before any of the
// real sections mount.
function emptyStateText(vizType: VizType): string {
  const { container, unmount } = render(
    <SharedDisplaySections
      vizType={vizType}
      resultColumns={[]}
      styleColumns={[]}
      tableState={{}}
      columnSettings={{}}
      conditionalFormatting={[]}
      onTableState={() => {}}
      onColumnSettings={() => {}}
      onConditionalFormatting={() => {}}
    />,
  );
  const text = container.querySelector("p")?.textContent ?? "";
  unmount();
  return text;
}

describe("SharedDisplaySections empty state", () => {
  it("names columns and running for a table with no result columns", () => {
    const text = emptyStateText("table");
    expect(text).toMatch(/column/i);
    expect(text).toMatch(/run/i);
  });

  // Load-bearing: a single shared sentence (the old "Run the chart to configure column
  // formatting.") would satisfy the table assertion above just as well — it mentions both
  // "column" and "run". That alone can't prove the message was tailored per viz. Comparing the
  // table's text against a cartesian viz's text forces them to actually diverge; a shared
  // sentence fails this test even though it would pass the first one.
  it("shows different empty-state wording for a cartesian viz than for a table", () => {
    const tableText = emptyStateText("table");
    const barText = emptyStateText("bar");
    expect(barText).not.toEqual(tableText);
  });

  it("names what a pivot table needs when it has no result columns", () => {
    const text = emptyStateText("pivot_table");
    expect(text).toMatch(/pivot|rows|columns/i);
  });
});
