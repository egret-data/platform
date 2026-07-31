import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SmartScalarChart } from "../SmartScalarChart";

const cols = [{ name: "mo", type: "varchar" }, { name: "rev", type: "double" }];

describe("SmartScalarChart", () => {
  it("shows the latest value, its period, and the % change + previous value", () => {
    render(<SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 120]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />);
    expect(screen.getByText("120")).toBeInTheDocument();
    expect(screen.getByText("Feb")).toBeInTheDocument();            // current period label
    expect(screen.getByText(/20\.00%/)).toBeInTheDocument();
    expect(screen.getByText(/vs previous: 100/)).toBeInTheDocument(); // previous value
  });
  it("marks a decrease with a down arrow", () => {
    const { container } = render(<SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 80]]} dimension="mo" metric="rev" />);
    expect(container.textContent).toContain("▼");
  });
  it("shows just the value when there is no prior period", () => {
    render(<SmartScalarChart columns={cols} rows={[["Jan", 100]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />);
    expect(screen.getByText("100")).toBeInTheDocument();
    expect(screen.queryByText(/vs/)).not.toBeInTheDocument();
  });
  it("renders an em dash when there is no data", () => {
    render(<SmartScalarChart columns={cols} rows={[]} dimension="mo" metric="rev" />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});

describe("SmartScalarChart — Metabase parity", () => {
  const cols = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  const rows = [["2026-02-01", 36860], ["2026-03-01", 36513]];

  // Metabase writes "vs. previous month", not a bare "vs. previous" — a reader should not have to
  // remember which granularity the chart was grouped by to know what it is compared against.
  it("names the period it compares against", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    expect(screen.getByText(/vs previous month:/)).toBeInTheDocument();
  });

  it("falls back to a bare 'vs previous' when the dimension carries no unit", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" />);
    expect(screen.getByText(/vs previous:/)).toBeInTheDocument();
  });

  // A metric where LESS is better — cost, churn, time to resolve — reads backwards otherwise:
  // green for the number going up. The same defect the gauge's fixed zones had before chart 04.
  it("can swap which direction is coloured good, without lying about the direction", () => {
    const { rerender, container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" />,
    );
    const trend = () => container.querySelector("[data-trend]") as HTMLElement;
    // The value fell, so by default it is red.
    const fell = trend().style.color;
    expect(trend().textContent).toContain("▼");

    rerender(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" switchPositiveNegative />);
    expect(trend().style.color).not.toBe(fell);
    // The ARROW still reports what happened — only the reading of it is inverted.
    expect(trend().textContent).toContain("▼");
  });

  it("compacts the numbers when asked", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" compact />);
    expect(screen.queryByText("36,513")).toBeNull();
    expect(screen.getByText(/36\.5k/i)).toBeInTheDocument();
  });

  // The period label should read at the grouping's granularity, as Metabase's "Apr 2029" does.
  it("formats the period label through the dimension's own format", () => {
    render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m"
        dimensionFormat={{ date_style: "MMMM YYYY" }} />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
  });
});
