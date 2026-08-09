import { describe, it, expect } from "vitest";
import { isDraftDirty, type DashboardDraft } from "../edit-draft";
import type { DraftCard, DraftTab } from "../tabs";

const card = (over: Partial<DraftCard> = {}): DraftCard => ({
  id: "c1", chart_id: "ch1", card_type: "chart",
  grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
  options: { autoHeight: false, hideEmpty: true }, tab_id: "server-t1", tabKey: "tab-0",
  ...over,
});

const tab = (over: Partial<DraftTab> = {}): DraftTab => ({ key: "tab-0", name: "Tab 1", position: 0, ...over });

const draft = (over: Partial<DashboardDraft> = {}): DashboardDraft => ({
  name: "Sales",
  width: "fixed",
  cards: [card(), card({ id: "c2", grid_col: 6 })],
  parameters: [
    { id: "p1", name: "Country", slug: "country", type: "category", operator: "is" },
    { id: "p2", name: "Date", slug: "date", type: "date", operator: "range" },
  ],
  tabs: [tab(), tab({ key: "tab-1", name: "Tab 2", position: 1 })],
  ...over,
});

describe("isDraftDirty", () => {
  it("is clean against an identical draft and against a deep copy of itself", () => {
    const base = draft();
    expect(isDraftDirty(base, base)).toBe(false);
    expect(isDraftDirty(base, JSON.parse(JSON.stringify(base)))).toBe(false);
  });

  it("is dirty on a rename, but not on leading or trailing whitespace alone", () => {
    // onSave writes `draftName.trim()` (DashboardView.tsx:416), so a draft that differs only by
    // surrounding whitespace would save byte-identical bytes. Prompting for that is a false alarm.
    expect(isDraftDirty(draft(), draft({ name: "Revenue" }))).toBe(true);
    expect(isDraftDirty(draft(), draft({ name: "  Sales " }))).toBe(false);
  });

  it("is dirty when the fixed/full width switch is flipped", () => {
    // NOT in the plan: W3 landed `draftWidth` into `startEdit`/`onSave` after this plan was
    // written, so width is now a fifth thing Cancel throws away. Leaving it out would make the
    // one edit W3 added the one edit that still gets discarded silently.
    expect(isDraftDirty(draft(), draft({ width: "full" }))).toBe(true);
  });

  it("is dirty when a card is added, removed, moved or resized", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [...base.cards, card({ id: "c3" })] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [base.cards[0]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ grid_row: 4 }), base.cards[1]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ size_y: 8 }), base.cards[1]] }))).toBe(true);
  });

  it("is dirty when a card's chart is replaced or its options change", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ chart_id: "ch9" }), base.cards[1]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ options: { autoHeight: true, hideEmpty: true } }), base.cards[1]] }))).toBe(true);
  });

  it("ignores card array order and options key order", () => {
    // duplicateCard and addCard append; positions live in grid_col/grid_row, so array order carries
    // no meaning. updateCardOptions spreads `{ ...c.options, ...patch }`, which reorders keys the
    // moment a patch overwrites an existing one — JSON.stringify preserves insertion order, so
    // without sorting, opening a settings panel and changing nothing would report a dirty board.
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [base.cards[1], base.cards[0]] }))).toBe(false);
    expect(isDraftDirty(base, draft({
      cards: [card({ options: { hideEmpty: true, autoHeight: false } }), base.cards[1]],
    }))).toBe(false);
  });

  it("ignores tab_id, which is the server id the draft does not edit", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ tab_id: "anything-else" }), base.cards[1]] }))).toBe(false);
  });

  it("is dirty when a card moves between draft tabs", () => {
    // moveCardToTab (tabs.ts:137-140) rewrites `tabKey` and leaves `tab_id` alone, so a comparison
    // that keyed on tab_id would call this unchanged — the exact inverse of the case above.
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ tabKey: "tab-1" }), base.cards[1]] }))).toBe(true);
  });

  it("is dirty when a parameter is added, removed, edited or REORDERED", () => {
    const base = draft();
    const [p1, p2] = base.parameters;
    expect(isDraftDirty(base, draft({ parameters: [p1] }))).toBe(true);
    expect(isDraftDirty(base, draft({ parameters: [p1, p2, { id: "p3", name: "F", slug: "f", type: "text", operator: "is" }] }))).toBe(true);
    expect(isDraftDirty(base, draft({ parameters: [{ ...p1, name: "Region" }, p2] }))).toBe(true);
    // Filter chips render in creation order (DashboardFilterBar.tsx), so a reorder is a real
    // change — the opposite rule from cards, and the reason parameters are not sorted.
    expect(isDraftDirty(base, draft({ parameters: [p2, p1] }))).toBe(true);
  });

  it("is dirty when a tab is renamed, added, deleted or reordered", () => {
    const base = draft();
    const [t1, t2] = base.tabs;
    expect(isDraftDirty(base, draft({ tabs: [{ ...t1, name: "Overview" }, t2] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [t1] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [...base.tabs, tab({ key: "tab-2", name: "Tab 3", position: 2 })] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [{ ...t1, position: 1 }, { ...t2, position: 0 }] }))).toBe(true);
  });
});
