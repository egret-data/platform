import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { CardSettingsPanel } from "../CardSettingsPanel";
import type { DraftCard } from "../tabs";
import type { Chart } from "@/api/charts";

// A dashboard picker that names a column `region` while the rest of the product says "Khu vực"
// is the same defect #444 closed inside the chart studio. The identity stays the machine name —
// what a row STORES keys on `column_name`, so a rename never rewrites a saved configuration.
const card = {
  id: "c1", chart_id: "ch1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 4, size_y: 5,
  options: { clickBehavior: { type: "crossfilter" } }, tab_id: null, tabKey: null,
} as unknown as DraftCard;

const chart = {
  id: "ch1", name: "Sales", dataset_id: "d1",
  params: { spec: { dimensions: ["region"], columns: ["city"], metrics: [] } },
} as unknown as Chart;

const render1 = (labels?: Record<string, string>) =>
  render(
    <CardSettingsPanel
      card={card} chart={chart} tabs={[]} labels={labels}
      parameters={[{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }]}
      onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()}
    />,
  );

describe("the crossfilter column picker", () => {
  it("shows the display name, not the machine name", () => {
    render1({ region: "Khu vực" });
    const opt = screen.getByRole("option", { name: "Khu vực" }) as HTMLOptionElement;
    // IDENTITY stays the machine name — the stored value must not become the label.
    expect(opt.value).toBe("region");
  });

  it("keeps the machine name reachable in the title, for whoever writes the SQL", () => {
    render1({ region: "Khu vực" });
    expect(screen.getByRole("option", { name: "Khu vực" })).toHaveAttribute("title", "Khu vực (region)");
  });

  it("falls back to the machine name when a column has no verbose_name", () => {
    render1({ region: "Khu vực" });
    const opt = screen.getByRole("option", { name: "city" }) as HTMLOptionElement;
    expect(opt.value).toBe("city");
    // and does not render the redundant `city (city)`
    expect(opt).toHaveAttribute("title", "city");
  });

  it("renders machine names when no labels are supplied at all", () => {
    render1(undefined);
    expect((screen.getByRole("option", { name: "region" }) as HTMLOptionElement).value).toBe("region");
  });
});
