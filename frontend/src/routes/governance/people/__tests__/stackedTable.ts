import { within } from "@testing-library/react";
import { expect } from "vitest";
import { STACKED_TABLE } from "@/routes/governance/tabParts";

/**
 * Live check D6: below `sm` the table stacks into cards, and each cell after
 * the first is labelled by its column (`data-label`, shown by CSS). jsdom
 * applies no media query, so this pins the two things the layout rests on:
 * the table carries `STACKED_TABLE`, and every cell's label is its column's
 * header. The pinned Actions column's buttons name themselves and carry none.
 */
export function expectStackedLabels(table: HTMLElement) {
  expect(table).toHaveClass(...STACKED_TABLE.split(" "));
  const headers = within(table)
    .getAllByRole("columnheader")
    .map((h) => h.textContent);
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows.length).toBeGreaterThan(0);
  for (const row of rows) {
    const labels = within(row)
      .getAllByRole("cell")
      .map((c) => c.getAttribute("data-label"));
    expect(labels).toEqual(headers.map((h, i) => (i === 0 || h === "Actions" ? null : h)));
  }
}
