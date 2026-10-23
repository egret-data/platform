import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

/** Renders the panel and opens the crossfilter picker; returns the options callback. */
async function open1(labels?: Record<string, string>, mapped?: string) {
  const onUpdateOptions = vi.fn();
  const shown = mapped === undefined ? card
    : { ...card, options: { clickBehavior: { type: "crossfilter", parameterMapping: { p1: mapped } } } };
  render(
    <CardSettingsPanel
      card={shown} chart={chart} tabs={[]} labels={labels}
      parameters={[{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }]}
      onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()}
    />,
  );
  await userEvent.setup().click(screen.getByRole("combobox", { name: "crossfilter Region" }));
  return onUpdateOptions;
}

/** Picks the option named `name` and returns the column the panel stored for the filter. */
async function storedFor(onUpdateOptions: ReturnType<typeof vi.fn>, name: string) {
  await userEvent.setup().click(await screen.findByRole("option", { name }));
  return onUpdateOptions.mock.lastCall?.[0].clickBehavior.parameterMapping.p1;
}

describe("the crossfilter column picker", () => {
  it("shows the display name, not the machine name", async () => {
    const onUpdateOptions = await open1({ region: "Khu vực" });
    // IDENTITY stays the machine name — the stored value must not become the label.
    expect(await storedFor(onUpdateOptions, "Khu vực")).toBe("region");
  });

  it("keeps the machine name reachable in the title, for whoever writes the SQL", async () => {
    await open1({ region: "Khu vực" });
    expect(await screen.findByRole("option", { name: "Khu vực" })).toHaveAttribute("title", "Khu vực (region)");
  });

  it("falls back to the machine name when a column has no verbose_name", async () => {
    const onUpdateOptions = await open1({ region: "Khu vực" });
    // and does not render the redundant `city (city)`
    expect(await screen.findByRole("option", { name: "city" })).toHaveAttribute("title", "city");
    expect(await storedFor(onUpdateOptions, "city")).toBe("city");
  });

  it("renders machine names when no labels are supplied at all", async () => {
    const onUpdateOptions = await open1(undefined);
    expect(await storedFor(onUpdateOptions, "region")).toBe("region");
  });

  it("stores the empty string for none, not the item's sentinel", async () => {
    const onUpdateOptions = await open1(undefined, "region");
    expect(await storedFor(onUpdateOptions, "none")).toBe("");
  });
});
