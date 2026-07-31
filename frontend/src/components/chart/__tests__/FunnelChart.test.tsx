import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { FunnelChart } from "../FunnelChart";

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];
const rows = [["Visited", 100], ["Signed up", 40], ["Paid", 10]];

describe("FunnelChart", () => {
  // The reason this stopped being an ECharts `funnel` series: that series can only draw its labels
  // INSIDE the shape, where a narrow step truncates them. Metabase puts the name above the column
  // and the value below it, outside.
  it("puts the names and the values outside the shape", () => {
    const { container } = render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    const bands = container.querySelectorAll("[data-funnel-band]");
    expect(bands.length).toBeGreaterThan(0);
    for (const band of bands) {
      expect(band.textContent).toBe("");
    }
    expect(screen.getAllByText("Visited").length).toBeGreaterThan(0);
    expect(screen.getByText("100")).toBeInTheDocument();
  });

  // Metabase emits N-1 polygons — one per ADJACENT PAIR — plus a column for the last step, so the
  // labels underneath stay under their own step.
  it("draws one band per step", () => {
    const { container } = render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    expect(container.querySelectorAll("[data-funnel-band]")).toHaveLength(3);
  });

  // A trapezoid's edges run from `value/max`, centred. For 100 then 40 out of a max of 100 that is
  // a left edge spanning the full height and a right edge spanning 0.3 to 0.7.
  it("shapes the first trapezoid from the two values it joins", () => {
    const { container } = render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    const pts = container.querySelector("[data-funnel-band] polygon")!.getAttribute("points")!;
    expect(pts).toBe("0,0 1,0.3 1,0.7 0,1");
  });

  // One colour for every step. A palette per step would say these are categories to compare; they
  // are stages of one flow narrowing.
  it("uses one colour for the whole funnel", () => {
    const { container } = render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    const fills = [...container.querySelectorAll("polygon")].map((p) => p.getAttribute("fill"));
    expect(new Set(fills).size).toBe(1);
  });

  // Metabase prints the metric's name under the first value — what the whole funnel is counting.
  it("names the metric under the first step only", () => {
    render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    expect(screen.getAllByText("cnt")).toHaveLength(1);
  });

  it("shows each later step's share of the first", () => {
    render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    expect(screen.getByText("40.00 %")).toBeInTheDocument();
    expect(screen.getByText("10.00 %")).toBeInTheDocument();
  });

  it("draws a column per step in bar mode", () => {
    const { container } = render(
      <FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" type="bar" />,
    );
    expect(container.querySelectorAll("polygon")).toHaveLength(0);
    expect(container.querySelectorAll("[data-funnel-band]")).toHaveLength(3);
  });

  it("says so rather than drawing an empty frame when there is nothing to draw", () => {
    render(<FunnelChart columns={cols} rows={[]} dimension="stage" metric="cnt" />);
    expect(screen.getByText("No data")).toBeInTheDocument();
  });
});
