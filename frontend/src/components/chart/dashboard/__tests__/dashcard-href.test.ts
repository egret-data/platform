import { describe, it, expect } from "vitest";
import type { DashboardCard } from "@/api/dashboards";
import { dashcardHref } from "../dashcard-href";

const card = (over: Partial<DashboardCard> = {}): DashboardCard => ({
  id: "k", chart_id: "ch1", card_type: "chart",
  grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id: null, ...over,
});

describe("dashcardHref", () => {
  it("points at the chart route the router actually declares", () => {
    // App.tsx:94 — `/explore/:workspace/charts/:id`. Read from the router, not guessed: a guessed
    // URL renders NotFound, which reads to a user as a permissions problem.
    expect(dashcardHref("demo", card(), false)).toBe("/explore/demo/charts/ch1");
  });

  it("is suppressed while editing", () => {
    expect(dashcardHref("demo", card(), true)).toBeUndefined();
  });

  it("is suppressed when the title override hides the header", () => {
    // "" is how a reader hides the header, and the link lives on the header.
    expect(dashcardHref("demo", card({ options: { title: "" } }), false)).toBeUndefined();
    expect(dashcardHref("demo", card({ options: { title: "Revenue" } }), false)).toBe("/explore/demo/charts/ch1");
    expect(dashcardHref("demo", card({ options: {} }), false)).toBe("/explore/demo/charts/ch1");
  });

  it("is suppressed for every virtual card kind and for a card with no chart", () => {
    for (const card_type of ["text", "heading", "link", "placeholder"]) {
      expect(dashcardHref("demo", card({ card_type }), false)).toBeUndefined();
    }
    expect(dashcardHref("demo", card({ chart_id: null }), false)).toBeUndefined();
  });
});
