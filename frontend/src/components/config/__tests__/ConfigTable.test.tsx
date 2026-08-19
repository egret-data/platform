import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";

import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";

describe("ConfigTableGroup", () => {
  it("spans the whole table, so a group heading is not mistaken for a Setting", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span /></ConfigTableRow>
      </ConfigTable>,
    );
    const cell = screen.getByText("Cache").closest("td");
    // colSpan is the point: a group row that occupied only the Setting column
    // would read as a setting with no value, which is what the cards were
    // replaced to avoid.
    expect(cell).toHaveAttribute("colspan", "3");
  });

  it("keeps ordinary rows in their three columns", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span data-testid="ctl" /></ConfigTableRow>
      </ConfigTable>,
    );
    expect(screen.getByText("Enabled").closest("td")).not.toHaveAttribute("colspan", "3");
    expect(screen.getByTestId("ctl")).toBeInTheDocument();
  });

  it("is announced as a heading for its group, not as data", () => {
    render(<ConfigTable><ConfigTableGroup label="Rate limit" /></ConfigTable>);
    // A screen reader hitting a bare cell mid-table learns nothing about
    // structure; the row has to say it starts a section.
    expect(screen.getByRole("rowheader", { name: "Rate limit" })).toBeInTheDocument();
  });
});
