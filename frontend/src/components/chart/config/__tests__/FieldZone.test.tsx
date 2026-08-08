import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
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

/**
 * THE COMPARISON RUNS BOTH WAYS. #441 gave the table's Data-tab list a filter past eight fields and
 * a bounded height, on the reasoning that a longer list stops being scannable by eye. The pivot's
 * "Add …" popover picks from exactly the same field set and got neither — measured on the kind
 * deployment against the seeded `MB Raw Orders` (ten columns): the table's list drew its "Filter 10
 * fields…" box and the pivot's popover, over the identical ten, drew none and ran to 330px with
 * `max-height: none`.
 *
 * The threshold is now one constant serving both, rather than the byte-for-byte second copy #441
 * spent a section removing from five files.
 */
describe("FieldZone's picker gets the affordances the table's list got", () => {
  const many = ["id", "created_at", "title", "category", "vendor", "subtotal", "discount", "total", "quantity", "state"];
  const openPicker = (options: string[], labels?: Record<string, string>) => {
    render(<FieldZone label="Rows" selected={[]} options={options} labels={labels} onChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "add rows" }));
  };

  it("filters the picker past the same threshold the table's list uses", () => {
    openPicker(many);
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "tot" } });
    expect(screen.getByRole("button", { name: /^subtotal$/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^total$/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^vendor$/ })).not.toBeInTheDocument();
  });

  it("finds a renamed field by its machine name as well as its displayed one", () => {
    openPicker([...many, "region"], { region: "Khu vực" });
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "region" } });
    expect(screen.getByText("Khu vực")).toBeInTheDocument();
  });

  it("says nothing matched rather than showing an empty picker", () => {
    openPicker(many);
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "zzz" } });
    expect(screen.getByText(/No field matches/)).toBeInTheDocument();
  });

  it("offers no filter on a short picker", () => {
    openPicker(["a", "b"]);
    expect(screen.queryByLabelText("Filter fields")).not.toBeInTheDocument();
  });

  /**
   * jsdom runs no layout, so what is pinned is the CLASS that bounds the list — the browser numbers
   * are in the PR body. The bound is a guard, not a fix for anything seen: the widest seeded dataset
   * is ten columns and the popover fits at 330px. A `PopoverContent` sets `max-height: none`, so at
   * thirty fields it would grow past the viewport exactly as #443's `DialogContent` did.
   */
  it("bounds the picker's list so a long one scrolls rather than growing", () => {
    openPicker(many);
    const list = screen.getByRole("button", { name: /^vendor$/ }).parentElement!;
    expect(list.className).toContain("overflow-y-auto");
    expect(list.className).toContain("max-h-64");
  });
});

describe("FieldZone labels reach the cartesian zones too", () => {
  it("a metric's verbose name shows in a Y-axis zone", () => {
    render(<FieldZone label="Y-axis" selected={["total_revenue"]} options={["total_revenue"]}
      labels={{ total_revenue: "Tổng doanh thu" }} onChange={vi.fn()} />);
    expect(screen.getByText("Tổng doanh thu")).toBeInTheDocument();
  });
});
