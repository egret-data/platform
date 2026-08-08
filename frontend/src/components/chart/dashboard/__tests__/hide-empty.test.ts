import { describe, it, expect } from "vitest";
import { hiddenEmptyCardIds } from "../hide-empty";
import type { DashboardCard } from "@/api/dashboards";

/**
 * `card.hide_empty` is Metabase's name for it; egret stores it as `options.hideEmpty`.
 *
 * The behaviour these assertions pin was MEASURED against Metabase 3033 on a scratch
 * dashboard (never dashboard 2 — that is the parity baseline), not read off the source:
 *
 *   - the whole TILE goes, not an empty state. With the flag off, an empty card draws a
 *     "No results!" sailboat; with it on there is no tile at all.
 *   - the row does NOT close up sideways. Bar | EMPTY | Row at cols 0/8/16 renders as
 *     Bar . . . . . Row with a hole in the middle, because react-grid-layout is given
 *     `compactType="vertical"`, which only pulls cards UP.
 *   - cards BELOW do move up. A card at row 6 with the row-0 card hidden drew at row 0.
 *
 * egret's own grid passes the same `compactType="vertical"` (DashboardGridLayout.tsx:64),
 * so dropping a card from `visibleCards` reproduces both halves for free. That is why this
 * module is only about WHICH ids to drop and owns no layout maths.
 */

const card = (id: string, options: Record<string, unknown> = {}, card_type = "chart"): DashboardCard => ({
  id,
  chart_id: "ch",
  card_type,
  grid_col: 0,
  grid_row: 0,
  size_x: 6,
  size_y: 4,
  options,
  tab_id: null,
});

describe("hiddenEmptyCardIds", () => {
  it("hides an opted-in card whose query came back with no rows", () => {
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], { c1: true }, false);
    expect([...ids]).toEqual(["c1"]);
  });

  it("keeps an opted-in card that has rows", () => {
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], { c1: false }, false);
    expect([...ids]).toEqual([]);
  });

  it("keeps an empty card that did NOT opt in", () => {
    const ids = hiddenEmptyCardIds([card("c1")], { c1: true }, false);
    expect([...ids]).toEqual([]);
  });

  it("hides nothing while editing, so the author can still see and configure the card", () => {
    // Metabase does the same: getVisibleCards returns every tab card when isEditing.
    // It also matters structurally — egret's onLayoutChange persists RGL's compacted
    // positions while editing, so hiding a card there would silently rewrite its
    // neighbours' saved grid_row.
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], { c1: true }, true);
    expect([...ids]).toEqual([]);
  });

  it("treats an unresolved card as empty, so it does not flash in and then vanish", () => {
    // Metabase's shouldHideCard returns `!wasVisible` while the data is loading, i.e. a
    // card nobody has seen yet stays hidden until its result arrives.
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], {}, false);
    expect([...ids]).toEqual(["c1"]);
  });

  it("keeps the last answer during a refetch rather than re-hiding a visible card", () => {
    // The probe stops reporting while isLoading, so the map still holds `false` here.
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], { c1: false }, false);
    expect([...ids]).toEqual([]);
  });

  it("ignores non-chart cards, which have no query to be empty", () => {
    // Metabase hides the toggle entirely for virtual dashcards (getHidden: isVirtualDashCard).
    const cards = [
      card("t1", { hideEmpty: true }, "text"),
      card("h1", { hideEmpty: true }, "heading"),
      card("l1", { hideEmpty: true }, "link"),
      card("p1", { hideEmpty: true }, "placeholder"),
    ];
    const ids = hiddenEmptyCardIds(cards, { t1: true, h1: true, l1: true, p1: true }, false);
    expect([...ids]).toEqual([]);
  });

  it("hides only the opted-in empty ones out of a mixed board", () => {
    const cards = [
      card("a", { hideEmpty: true }),
      card("b", { hideEmpty: true }),
      card("c", {}),
      card("d", { hideEmpty: true }),
    ];
    const ids = hiddenEmptyCardIds(cards, { a: true, b: false, c: true, d: true }, false);
    expect([...ids].sort()).toEqual(["a", "d"]);
  });

  it("returns an empty set when no card opted in, so the caller can skip the filter", () => {
    const ids = hiddenEmptyCardIds([card("a"), card("b")], { a: true, b: true }, false);
    expect(ids.size).toBe(0);
  });

  it("does not hide a chart card whose chart is gone, which would hide the breakage", () => {
    // No chart_id means no query, so no probe runs for it and `emptiness` would stay
    // absent forever — the `?? true` default would hide it permanently. The tile's own
    // "Chart not found" is the correct thing to show, exactly as an ERRORED card is not
    // hidden either.
    const orphan = { ...card("c1", { hideEmpty: true }), chart_id: null };
    const ids = hiddenEmptyCardIds([orphan], {}, false);
    expect([...ids]).toEqual([]);
  });

  it("hides nothing while a required filter is unset, because no card ran at all", () => {
    // With a required-but-unset parameter every card is `blocked` and renders "Set the
    // required filters to load this card" instead of querying. Nothing reports, so the
    // `?? true` default would wrongly hide every opted-in card behind that prompt.
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], {}, false, true);
    expect([...ids]).toEqual([]);
  });

  it("still hides a card that really is empty once the filters are satisfied", () => {
    const ids = hiddenEmptyCardIds([card("c1", { hideEmpty: true })], { c1: true }, false, false);
    expect([...ids]).toEqual(["c1"]);
  });
});
