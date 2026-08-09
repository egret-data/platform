import { describe, it, expect } from "vitest";
import { startTabsDraft, serializeLayout, addTab, renameTab, reorderTab, deleteTab, duplicateTab, moveCardToTab, type DraftCard, type DraftTab } from "../tabs";
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

describe("addTab", () => {
  it("first add on a tabless dashboard splits existing cards onto Tab 1 + empty Tab 2", () => {
    const draftCards: DraftCard[] = [{ ...card("k1", null), tabKey: null }];
    const r = addTab([], draftCards);
    expect(r.draftTabs.map((t) => t.name)).toEqual(["Tab 1", "Tab 2"]);
    const k1Tab = r.draftTabs[0].key;
    expect(r.draftCards.find((c) => c.id === "k1")!.tabKey).toBe(k1Tab);
    expect(r.activeTabPos).toBe(1);
  });
  it("subsequent add appends one empty tab and activates it", () => {
    const start = addTab([], [{ ...card("k1", null), tabKey: null }]);
    const r = addTab(start.draftTabs, start.draftCards);
    expect(r.draftTabs.map((t) => t.name)).toEqual(["Tab 1", "Tab 2", "Tab 3"]);
    expect(r.activeTabPos).toBe(2);
  });
});

describe("renameTab", () => {
  it("renames by key", () => {
    const start = addTab([], [{ ...card("k1", null), tabKey: null }]);
    const out = renameTab(start.draftTabs, start.draftTabs[0].key, "Sales");
    expect(out[0].name).toBe("Sales");
    expect(out[1].name).toBe("Tab 2");
  });
});

describe("reorderTab", () => {
  it("moves a tab from one position to another and renumbers", () => {
    const start = addTab(addTab([], [{ ...card("k1", null), tabKey: null }]).draftTabs, []);
    const out = reorderTab(start.draftTabs, 0, 2);
    expect(out.map((t) => t.name)).toEqual(["Tab 2", "Tab 3", "Tab 1"]);
    expect(out.map((t) => t.position)).toEqual([0, 1, 2]);
  });
});

const positioned = (id: string, tabKey: string | null, row: number, h = 4): DraftCard =>
  ({ ...card(id, null), tabKey, grid_row: row, size_y: h });

describe("deleteTab", () => {
  it("reassign mode moves the tab's cards to the target tab", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const cards = [positioned("k1", "ka", 0), positioned("k2", "kb", 0)];
    const out = deleteTab(tabs, cards, "ka", { mode: "reassign", toKey: "kb" });
    expect(out.draftTabs.map((t) => t.name)).toEqual(["B"]);
    expect(out.draftCards.every((c) => c.tabKey === "kb")).toBe(true);
  });
  it("delete-cards mode drops the tab and its cards", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }, { key: "kb", name: "B", position: 1 }];
    const cards = [positioned("k1", "ka", 0), positioned("k2", "kb", 0)];
    const out = deleteTab(tabs, cards, "ka", { mode: "delete" });
    expect(out.draftCards.map((c) => c.id)).toEqual(["k2"]);
  });
});

describe("duplicateTab", () => {
  it("clones the tab's cards into a new tab with fresh ids", () => {
    const tabs: DraftTab[] = [{ key: "ka", name: "A", position: 0 }];
    const cards = [positioned("k1", "ka", 0)];
    const out = duplicateTab(tabs, cards, "ka");
    expect(out.draftTabs).toHaveLength(2);
    const newKey = out.draftTabs[1].key;
    const clones = out.draftCards.filter((c) => c.tabKey === newKey);
    expect(clones).toHaveLength(1);
    expect(clones[0].id).not.toBe("k1");
    expect(out.activeTabPos).toBe(1);
  });
});

describe("moveCardToTab", () => {
  it("reassigns a card and stacks it below the destination's max row", () => {
    const cards = [positioned("k1", "ka", 0, 4), positioned("k2", "kb", 0, 4)];
    const out = moveCardToTab(cards, "k1", "kb");
    const moved = out.find((c) => c.id === "k1")!;
    expect(moved.tabKey).toBe("kb");
    expect(moved.grid_row).toBe(4);
  });
});

describe("duplicateTab and a card's inline filters", () => {
  it("does not let a clone claim the original's filters", () => {
    // Two cards holding the same parameter id is not a state the model can render:
    // findHostCardId picks the first, the clone draws nothing, and the dashboard carries an
    // invisible inconsistency. Metabase duplicates the PARAMETERS instead (cards-typed.ts:416-441);
    // that forks the URL contract with a fresh slug and is deliberately out of scope here.
    const tabs: DraftTab[] = [{ key: "t1", name: "Tab 1", position: 0 }];
    const cards: DraftCard[] = [
      { id: "a", chart_id: "c", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 12, size_y: 6,
        options: { title: "Revenue", inlineParameters: ["p1"] }, tab_id: null, tabKey: "t1" },
    ];
    const r = duplicateTab(tabs, cards, "t1");
    const clone = r.draftCards.find((c) => c.id !== "a")!;
    expect(clone.options).toEqual({ title: "Revenue" });
    expect(r.draftCards.find((c) => c.id === "a")!.options).toEqual({ title: "Revenue", inlineParameters: ["p1"] });
  });

  it("leaves a card that hosts nothing byte-for-byte alone", () => {
    // `withoutInlineParameters` returns the SAME object when the key is absent, so a tab of
    // ordinary cards is cloned exactly as it was before this change.
    const tabs: DraftTab[] = [{ key: "t1", name: "Tab 1", position: 0 }];
    const options = { title: "Revenue", hideEmpty: true };
    const cards: DraftCard[] = [
      { id: "a", chart_id: "c", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 12, size_y: 6,
        options, tab_id: null, tabKey: "t1" },
    ];
    const r = duplicateTab(tabs, cards, "t1");
    expect(r.draftCards.find((c) => c.id !== "a")!.options).toBe(options);
  });
});
