import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { FieldZone } from "../FieldZone";

describe("FieldZone display names", () => {
  it("shows a field's verbose name instead of its machine name", () => {
    // Datasets have carried `verbose_name` for a long time and offer a UI to set it, but the chart
    // studio read only the machine name — so naming a column "Total Sales" changed nothing here.
    render(<FieldZone label="Values" selected={["total_revenue"]} options={["total_revenue"]}
      labels={{ total_revenue: "Total Sales" }} onChange={vi.fn()} />);
    expect(screen.getByText("Total Sales")).toBeInTheDocument();
    expect(screen.queryByText("total_revenue")).not.toBeInTheDocument();
  });

  it("falls back to the machine name when there is no verbose name", () => {
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]} onChange={vi.fn()} />);
    expect(screen.getByText("region")).toBeInTheDocument();
  });

  it("treats a blank verbose name as absent", () => {
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]}
      labels={{ region: "   " }} onChange={vi.fn()} />);
    expect(screen.getByText("region")).toBeInTheDocument();
  });

  it("names the field in the drag and remove controls, not just 'drag'", () => {
    // Four identical "drag" buttons tell a screen-reader user nothing about which row they are on.
    render(<FieldZone label="Rows" selected={["region"]} options={["region"]}
      labels={{ region: "Region" }} onChange={vi.fn()} />);
    expect(screen.getByRole("button", { name: "reorder Region" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "remove Region" })).toBeInTheDocument();
  });
});

/**
 * The zone's own controls, measured live in the pivot's 255px panel before this:
 *   drag grip  14 × 14
 *   remove ×    8 × 20
 *   "Add rows…" 255 × 26
 * against WCAG 2.2 SC 2.5.8's 24 × 24 minimum, in a chip with ~200px of inert space between the
 * two. jsdom measures nothing, so what is pinned here is the sizing class each control carries —
 * `size-6` is 24×24, `min-h-8` is 32 tall — and the browser numbers are in the PR body.
 */
describe("FieldZone controls are big enough to hit", () => {
  const zone = () => render(<FieldZone label="Rows" selected={["region"]} options={["region", "state"]}
    labels={{ region: "Region" }} onChange={vi.fn()} />);

  it("gives the drag grip and the remove button a 24px target around their icons", () => {
    zone();
    expect(screen.getByRole("button", { name: "reorder Region" }).className).toContain("size-6");
    expect(screen.getByRole("button", { name: "remove Region" }).className).toContain("size-6");
  });

  it("gives the add affordance a full row", () => {
    zone();
    expect(screen.getByRole("button", { name: "add rows" }).className).toContain("min-h-8");
  });

  /** A zone with nothing selected and nothing left to offer used to draw its heading over blank
   *  space — live, the pivot's Values zone on a dataset that defines no metrics. */
  it("explains an empty zone rather than heading blank space", () => {
    render(<FieldZone label="Values" selected={[]} options={[]} onChange={vi.fn()} />);
    expect(screen.getByText(/Nothing available for values/i)).toBeInTheDocument();
  });

  /** The heading is the zone's accessible name, so the chips inside it are announced as belonging
   *  somewhere. It was a bare `<div>` tied to nothing. */
  it("names the group its chips belong to", () => {
    zone();
    expect(screen.getByRole("group", { name: "Rows" })).toBeInTheDocument();
  });
});

describe("FieldZone labels reach the cartesian zones too", () => {
  it("a metric's verbose name shows in a Y-axis zone", () => {
    render(<FieldZone label="Y-axis" selected={["total_revenue"]} options={["total_revenue"]}
      labels={{ total_revenue: "Tổng doanh thu" }} onChange={vi.fn()} />);
    expect(screen.getByText("Tổng doanh thu")).toBeInTheDocument();
  });
});
