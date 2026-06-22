import { describe, it, expect } from "vitest";
import { startTabsDraft, serializeLayout, type DraftCard, type DraftTab } from "../tabs";
import type { DashboardCard, DashboardTab } from "@/api/dashboards";

const card = (id: string, tab_id: string | null): DashboardCard => ({
  id, chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id,
});

describe("startTabsDraft", () => {
  it("maps server tab_id to a draft tab key", () => {
    const tabs: DashboardTab[] = [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }];
    const cards = [card("k1", "t1"), card("k2", "t2")];
    const { draftTabs, draftCards } = startTabsDraft(tabs, cards);
    expect(draftTabs.map((t) => t.name)).toEqual(["A", "B"]);
    const keyA = draftTabs[0].key, keyB = draftTabs[1].key;
    expect(draftCards.find((c) => c.id === "k1")!.tabKey).toBe(keyA);
    expect(draftCards.find((c) => c.id === "k2")!.tabKey).toBe(keyB);
  });
  it("a tabless dashboard yields no tabs and null tabKeys", () => {
    const { draftTabs, draftCards } = startTabsDraft([], [card("k1", null)]);
    expect(draftTabs).toEqual([]);
    expect(draftCards[0].tabKey).toBeNull();
  });
});

describe("serializeLayout", () => {
  it("emits tabs by position and resolves each card's tab_index", () => {
    const draftTabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const draftCards: DraftCard[] = [
      { ...card("k1", null), tabKey: "kb" },
      { ...card("k2", null), tabKey: "ka" },
    ];
    const out = serializeLayout(draftTabs, draftCards);
    expect(out.tabs).toEqual([{ name: "A", position: 0 }, { name: "B", position: 1 }]);
    expect(out.cards.find((c) => c.tab_index === 0)).toBeTruthy();
    expect(out.cards.find((c) => c.tab_index === 1)).toBeTruthy();
  });
  it("tabless: tabs empty and every tab_index null", () => {
    const draftCards: DraftCard[] = [{ ...card("k1", null), tabKey: null }];
    const out = serializeLayout([], draftCards);
    expect(out.tabs).toEqual([]);
    expect(out.cards[0].tab_index).toBeNull();
  });
});
