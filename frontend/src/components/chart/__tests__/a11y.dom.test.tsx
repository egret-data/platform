import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { BigNumber } from "../BigNumber";
import { ProgressChart } from "../ProgressChart";
import { SmartScalarChart } from "../SmartScalarChart";

/**
 * The DOM-rendered charts have no canvas and so no ECharts `aria` to lean on: whatever a
 * screen reader gets is whatever the markup says. Before the audit that was a bare number
 * followed by a loose word.
 */

const columns = [{ name: "total_revenue", type: "double" }];

describe("BigNumber", () => {
  it("announces the metric together with its value", () => {
    render(<BigNumber columns={columns} rows={[[25468]]} metric="total_revenue" />);
    expect(screen.getByRole("img", { name: "total_revenue: 25,468" })).toBeInTheDocument();
  });

  it("says there is no value rather than labelling itself with an em dash", () => {
    // No metric means no column to read, so the viz shows "—"; that is not something a
    // screen reader should announce.
    render(<BigNumber columns={columns} rows={[[25468]]} />);
    expect(screen.getByRole("img", { name: "No value" })).toBeInTheDocument();
  });
});

describe("ProgressChart", () => {
  it("is a progressbar with its value, range and a spoken form", () => {
    render(<ProgressChart columns={columns} rows={[[75]]} metric="total_revenue" goal={100} />);
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "75");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    expect(bar).toHaveAttribute("aria-valuetext", "75 of 100");
  });

  it("says so when there is no value", () => {
    render(<ProgressChart columns={columns} rows={[]} metric="total_revenue" goal={100} />);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuetext", "no value");
  });
});

describe("SmartScalarChart", () => {
  it("announces the value and the direction of the change in one label", () => {
    const cols = [{ name: "month", type: "varchar" }, { name: "total_revenue", type: "double" }];
    render(
      <SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 150]]}
        dimension="month" metric="total_revenue" />,
    );
    const label = screen.getByRole("img").getAttribute("aria-label") ?? "";
    expect(label).toContain("150");
    expect(label).toMatch(/up/);
    // MOVED: was `previous 100`. There is no temporal unit here, so the default comparison is
    // `previousValue`, and that arm now names the row it reached rather than saying "previous".
    // Same correction as `SmartScalarChart.test.tsx`'s — the accessible name tracks the caption,
    // which is the property this test is really for.
    expect(label).toContain("versus Jan, 100");
  });
});
