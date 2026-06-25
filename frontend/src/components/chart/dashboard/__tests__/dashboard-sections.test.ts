import { describe, it, expect } from "vitest";
import { layoutOptions, dropPlaceholders } from "../dashboard-sections";
import type { DraftCard } from "../tabs";

const byId = Object.fromEntries(layoutOptions.map((l) => [l.id, l]));

describe("dashboard-sections layouts", () => {
  it("kpi_grid: heading + 2x2 of 12x5 placeholders", () => {
    expect(byId.kpi_grid.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 1, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 0, grid_row: 6, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 6, size_x: 12, size_y: 5 },
    ]);
  });

  it("large_chart_kpi_right: heading + 18x9 large + 3 right 6x3", () => {
    expect(byId.large_chart_kpi_right.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 18, size_y: 9 },
      { card_type: "placeholder", grid_col: 18, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 4, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 7, size_x: 6, size_y: 3 },
    ]);
  });

  it("kpi_chart_below: heading + 4 top 6x3 + 24x9 large", () => {
    expect(byId.kpi_chart_below.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 6, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 12, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 0, grid_row: 4, size_x: 24, size_y: 9 },
    ]);
  });

  it("respects the origin offset (heading at row, body below)", () => {
    expect(byId.kpi_grid.build({ col: 0, row: 7 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 7, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 8, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 8, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 0, grid_row: 13, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 13, size_x: 12, size_y: 5 },
    ]);
  });

  it("dropPlaceholders removes only placeholder cards", () => {
    const cards = [
      { id: "a", card_type: "chart" },
      { id: "b", card_type: "placeholder" },
      { id: "c", card_type: "heading" },
    ] as unknown as DraftCard[];
    expect(dropPlaceholders(cards).map((c) => c.id)).toEqual(["a", "c"]);
  });
});
