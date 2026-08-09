import { describe, it, expect } from "vitest";
import type { DashboardParameter } from "@/api/dashboards";
import { moveCardToTab } from "../tabs";
import {
  INLINE_PARAM_BUFFER_PX,
  INLINE_PARAM_HEADING_BUFFER_PX,
  destinationLabel,
  dropParameters,
  findHostCardId,
  hostTabKey,
  inlineFilterGutterClass,
  inlineParameterIds,
  inlineParametersFor,
  keepInlineParameters,
  mergeTopBarOrder,
  moveDestinations,
  moveParameterToCard,
  removeCardAndHostedParameters,
  shouldCollapseParameterList,
  supportsInlineParameters,
  topBarParameters,
  withoutInlineParameters,
} from "../inlineParameters";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

const params: DashboardParameter[] = [
  p({ id: "p1", name: "Region", slug: "region" }),
  p({ id: "p2", name: "City", slug: "city" }),
  p({ id: "p3", name: "Owner", slug: "owner" }),
];

interface TestCard {
  id: string;
  card_type: string;
  chart_id: string | null;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
  tab_id: string | null;
  tabKey: string | null;
}
const card = (over: Partial<TestCard>): TestCard => ({
  id: "c1", card_type: "chart", chart_id: "ch1", grid_col: 0, grid_row: 0, size_x: 12, size_y: 6,
  options: {}, tab_id: null, tabKey: "t1", ...over,
});

const cards: TestCard[] = [
  card({ id: "head", card_type: "heading", chart_id: null, options: { text: "Sales" } }),
  card({ id: "bar", options: { title: "Revenue", inlineParameters: ["p1"] } }),
  card({ id: "far", tabKey: "t2", options: { inlineParameters: ["p3"] } }),
  card({ id: "note", card_type: "text", chart_id: null, options: { text: "hi" } }),
];

describe("supportsInlineParameters", () => {
  it("admits egret's question and heading cards and nothing else", () => {
    // Ported from Metabase utils.ts:118-122 (isQuestionDashCard || isHeadingDashCard) onto
    // egret's card_type vocabulary (DashboardCardView.tsx:116-126).
    expect(supportsInlineParameters("chart")).toBe(true);
    expect(supportsInlineParameters("heading")).toBe(true);
    expect(supportsInlineParameters("text")).toBe(false);
    expect(supportsInlineParameters("link")).toBe(false);
    expect(supportsInlineParameters("placeholder")).toBe(false);
  });
});

describe("inlineParameterIds", () => {
  it("reads the raw array and tolerates every other shape the bag could hold", () => {
    expect(inlineParameterIds({ options: { inlineParameters: ["p1", "p2"] } })).toEqual(["p1", "p2"]);
    expect(inlineParameterIds({ options: {} })).toEqual([]);
    expect(inlineParameterIds({ options: { inlineParameters: "p1" } })).toEqual([]);
    expect(inlineParameterIds({ options: { inlineParameters: ["p1", 7, null] } })).toEqual(["p1"]);
  });
});

describe("findHostCardId", () => {
  it("finds the host on ANY tab, not just the one being viewed", () => {
    // selectors.ts:223-226 — getCurrentDashcards is every dashcard on the dashboard. A per-tab
    // lookup makes a filter appear and vanish as you switch tabs, which reads as a bug.
    expect(findHostCardId("p1", cards)).toBe("bar");
    expect(findHostCardId("p3", cards)).toBe("far");
  });

  it("returns null for a parameter no card claims", () => {
    expect(findHostCardId("p2", cards)).toBeNull();
  });
});

describe("hostTabKey", () => {
  it("names the tab of the card hosting the filter, which is what turns on the other-tab hint", () => {
    // W2 built `cardMapState`'s `other-tab` branch and left `filterTabKey: undefined` because
    // egret filters had no owning card. This is the value that activates it
    // (card-map-state.ts:54-63, DashCardCardParameterMapperContent.tsx:86-92).
    expect(hostTabKey("p3", cards)).toBe("t2");
    expect(hostTabKey("p1", cards)).toBe("t1");
  });

  it("is undefined for a top-bar filter, so the hint stays inert exactly as it is today", () => {
    // `undefined`, not `null`: `cardMapState` guards with `filterTabKey != null`, and a HOSTED
    // card on a tabless dashboard legitimately has a null tabKey — that must not fire either.
    expect(hostTabKey("p2", cards)).toBeUndefined();
    const tabless = [card({ id: "solo", tabKey: null, options: { inlineParameters: ["p1"] } })];
    expect(hostTabKey("p1", tabless)).toBeNull();
  });

  it("normalises a host with no tabKey AT ALL to null, so `undefined` means exactly one thing", () => {
    // `InlineHostCard.tabKey` is optional, and a view-mode `DashboardCard` genuinely has no such
    // key (only `tab_id`). `host?.tabKey` would hand that card back `undefined` — the same answer
    // as "no card hosts this filter" — and collapse the two states this function exists to
    // distinguish. Both are inert in `cardMapState` today, which is precisely why the contract
    // has to be asserted here rather than left to the caller's `!= null`.
    const noTabKey: { id: string; card_type: string; options: Record<string, unknown> }[] = [
      { id: "solo", card_type: "chart", options: { inlineParameters: ["p1"] } },
    ];
    expect(hostTabKey("p1", noTabKey)).toBeNull();
    expect(hostTabKey("p2", noTabKey)).toBeUndefined();
  });
});

describe("topBarParameters", () => {
  it("drops every hosted parameter, including one hosted on another tab, and keeps the order", () => {
    expect(topBarParameters(params, cards).map((x) => x.id)).toEqual(["p2"]);
    expect(topBarParameters(params, []).map((x) => x.id)).toEqual(["p1", "p2", "p3"]);
  });
});

describe("inlineParametersFor", () => {
  it("returns the card's own parameters in the CARD's order, not the dashboard's", () => {
    const both = card({ id: "bar", options: { inlineParameters: ["p3", "p1"] } });
    expect(inlineParametersFor(both, params).map((x) => x.id)).toEqual(["p3", "p1"]);
  });

  it("skips an id no parameter answers to, rather than rendering a hole", () => {
    const stale = card({ id: "bar", options: { inlineParameters: ["p1", "gone"] } });
    expect(inlineParametersFor(stale, params).map((x) => x.id)).toEqual(["p1"]);
  });
});

describe("moveParameterToCard", () => {
  it("moves a top-bar parameter onto a card", () => {
    const next = moveParameterToCard(cards, "p2", "head");
    expect(inlineParameterIds(next.find((c) => c.id === "head")!)).toEqual(["p2"]);
  });

  it("moves a hosted parameter back to the top of the page", () => {
    const next = moveParameterToCard(cards, "p1", null);
    expect(inlineParameterIds(next.find((c) => c.id === "bar")!)).toEqual([]);
    expect(findHostCardId("p1", next)).toBeNull();
  });

  it("moves between cards without leaving it on both", () => {
    const next = moveParameterToCard(cards, "p1", "head");
    expect(inlineParameterIds(next.find((c) => c.id === "bar")!)).toEqual([]);
    expect(inlineParameterIds(next.find((c) => c.id === "head")!)).toEqual(["p1"]);
  });

  it("refuses a destination that cannot host, leaving the array untouched by reference", () => {
    // A text card has no header row to draw a filter in. Silently accepting the move would
    // make the filter vanish from the top bar AND from the board.
    expect(moveParameterToCard(cards, "p2", "note")).toBe(cards);
  });
});

describe("mergeTopBarOrder", () => {
  it("applies a reordered top bar without losing the hosted parameters", () => {
    // THE BUG THIS EXISTS TO PREVENT: W6's drag-reorder writes the whole draft array back
    // (`onReorder={setDraftParameters}`). Handing it a list that had the inline parameters
    // filtered out would DELETE them.
    const merged = mergeTopBarOrder(params, [p({ id: "p2" })]);
    expect(merged.map((x) => x.id).sort()).toEqual(["p1", "p2", "p3"]);
    expect(merged[0].id).toBe("p2");
  });

  it("keeps the reordered entries, not the originals", () => {
    const renamed = mergeTopBarOrder(params, [p({ id: "p2", name: "Town" })]);
    expect(renamed.find((x) => x.id === "p2")!.name).toBe("Town");
  });
});

describe("dropParameters", () => {
  it("removes the named parameters and every reference to them", () => {
    // Metabase's removeParameterAndReferences (parameters.tsx:357-376).
    const linked = [p({ id: "p1" }), p({ id: "p2", filteringParameters: ["p1", "p3"] })];
    expect(dropParameters(linked, new Set(["p1"]))).toEqual([
      p({ id: "p2", filteringParameters: ["p3"] }),
    ]);
  });

  it("returns the same array reference when nothing is dropped", () => {
    expect(dropParameters(params, new Set())).toBe(params);
  });
});

describe("removeCardAndHostedParameters", () => {
  it("deletes the card's filters WITH the card, and every reference to them", () => {
    // Metabase does exactly this (cards-typed.ts:490-495 calls removeParameterAndReferences per
    // inline parameter). Returning them to the top bar instead would make filters materialise out
    // of a DELETE, which is the surprise that behaviour exists to avoid.
    const linked = [...params, p({ id: "p4", filteringParameters: ["p1"] })];
    const r = removeCardAndHostedParameters(cards, linked, "bar");
    expect(r.cards.map((c) => c.id)).toEqual(["head", "far", "note"]);
    expect(r.parameters.map((x) => x.id)).toEqual(["p2", "p3", "p4"]);
    expect(r.parameters.find((x) => x.id === "p4")!.filteringParameters).toEqual([]);
  });

  it("leaves the parameter array BY REFERENCE when the card hosted none", () => {
    // The caller writes no parameter draft in that case, so removing an ordinary card cannot
    // touch the filters at all — and `undoableDraftEdit`'s snapshot stays meaningful.
    const r = removeCardAndHostedParameters(cards, params, "note");
    expect(r.parameters).toBe(params);
    expect(r.cards.map((c) => c.id)).toEqual(["head", "bar", "far"]);
  });

  it("is a no-op on a card id that is not there", () => {
    const r = removeCardAndHostedParameters(cards, params, "ghost");
    expect(r.cards).toHaveLength(4);
    expect(r.parameters).toBe(params);
  });
});

describe("keepInlineParameters / withoutInlineParameters", () => {
  it("keep preserves only the placement across a Replace chart", () => {
    expect(keepInlineParameters({ title: "T", inlineParameters: ["p1"] })).toEqual({ inlineParameters: ["p1"] });
    expect(keepInlineParameters({ title: "T" })).toEqual({});
  });

  it("without strips the placement for a clone", () => {
    expect(withoutInlineParameters({ title: "T", inlineParameters: ["p1"] })).toEqual({ title: "T" });
    expect(withoutInlineParameters({ title: "T" })).toEqual({ title: "T" });
  });

  it("without returns the SAME options object when there is nothing to strip", () => {
    // Reference identity, not deep equality. `duplicateTab` and `duplicateCard` both run this over
    // every clone; rebuilding an untouched bag would churn `options` on every ordinary card and
    // make `isDraftDirty` (edit-draft.ts) report a dirty draft nobody edited — which is what pops
    // W2's discard confirmation on a Cancel that should have exited straight away.
    const options = { title: "T", hideEmpty: true };
    expect(withoutInlineParameters(options)).toBe(options);
  });
});

describe("destinationLabel", () => {
  it("names a heading by its text and a chart by its title, then its chart name", () => {
    expect(destinationLabel({ card_type: "heading", options: { text: "Sales" } }, undefined)).toBe("Sales");
    expect(destinationLabel({ card_type: "heading", options: { text: "  " } }, undefined)).toBe("Heading");
    expect(destinationLabel({ card_type: "chart", options: { title: "Revenue" } }, "Orders")).toBe("Revenue");
    expect(destinationLabel({ card_type: "chart", options: {} }, "Orders")).toBe("Orders");
    expect(destinationLabel({ card_type: "chart", options: {} }, undefined)).toBe("Card");
  });
});

describe("moveDestinations", () => {
  const tabs = [{ key: "t1", name: "Overview", position: 0 }, { key: "t2", name: "Detail", position: 1 }];
  const chartNameOf = (id: string) => (id === "bar" ? "Orders" : "");

  it("groups hostable cards by tab, in tab order, skipping non-hostable cards", () => {
    expect(moveDestinations({ cards, tabs, chartNameOf })).toEqual([
      { cardId: "head", label: "Sales", tabName: "Overview" },
      { cardId: "bar", label: "Revenue", tabName: "Overview" },
      { cardId: "far", label: "Card", tabName: "Detail" },
    ]);
  });

  it("emits ungrouped destinations on a dashboard with no tabs", () => {
    // MoveParameterMenu.tsx:117-124 pushes the cards into the root group when no tab has any.
    expect(moveDestinations({ cards, tabs: [], chartNameOf }).map((d) => d.tabName))
      .toEqual([null, null, null]);
  });
});

describe("shouldCollapseParameterList", () => {
  it("is Metabase's arithmetic: reserved + list + buffer > container", () => {
    // use-responsive-parameter-list.ts:29-34. Fed as numbers because jsdom measures nothing.
    //
    // 120 + 200 + 100 = 420 required. The comparison is STRICTLY greater, so a container of
    // exactly 420 fits and does NOT collapse — that equality case is the one that kills a `>=`
    // mutant, and it is why the boundary is asserted on all three sides. (This workstream's plan
    // wrote the 420 case as `true`; the plan was wrong, and Metabase's `>` is what ships.)
    const m = { reservedWidth: 120, listWidth: 200, bufferPx: INLINE_PARAM_BUFFER_PX };
    expect(shouldCollapseParameterList({ ...m, containerWidth: 421 })).toBe(false);
    expect(shouldCollapseParameterList({ ...m, containerWidth: 420 })).toBe(false);
    expect(shouldCollapseParameterList({ ...m, containerWidth: 419 })).toBe(true);
  });

  it("stays expanded before anything has been measured", () => {
    // First paint and jsdom both report 0. Metabase returns false when a ref is missing
    // (use-responsive-parameter-list.ts:25-27); starting collapsed would flash a
    // "Show filters" button on every card on every load.
    expect(shouldCollapseParameterList({ containerWidth: 0, reservedWidth: 0, listWidth: 0, bufferPx: 100 })).toBe(false);
  });

  it("carries Metabase's two buffers", () => {
    expect(INLINE_PARAM_BUFFER_PX).toBe(100);          // DashCardVisualization.tsx:445
    expect(INLINE_PARAM_HEADING_BUFFER_PX).toBe(24);   // use-responsive-parameter-list.ts:13 default
  });
});

describe("inlineFilterGutterClass", () => {
  it("reserves the width of W5's hover cluster, which is TWO buttons and not one", () => {
    // `ui/button.tsx:66` — `size="sm"` around one 16px glyph is `has-[>svg]:px-2.5`, so
    // 10 + 16 + 10 = 36px. W5 put the Edit-question pencil beside the export menu with a
    // `gap-0.5` (2px), at `right-1` (4px): 4 + 36 + 2 + 36 = 78 → pr-20 (80px).
    // The pre-W5 header had one button, which is where this workstream's plan's `pr-10` came
    // from — it is now the ONE-button case, not the general one.
    expect(inlineFilterGutterClass(2)).toBe("pr-20");
    expect(inlineFilterGutterClass(1)).toBe("pr-10");
    expect(inlineFilterGutterClass(0)).toBe("pr-4");
  });
});

describe("a host card carries its filters to another tab", () => {
  it("moveCardToTab preserves options.inlineParameters", () => {
    // Metabase keeps inline_parameters through a tab move (parameters.tsx:279-291 even follows
    // the card to its tab). egret gets this free because moveCardToTab spreads {...c} — this
    // test is what makes a future refactor that rebuilds `options` fail loudly.
    const moved = moveCardToTab(cards as never, "bar", "t2");
    expect(inlineParameterIds(moved.find((c) => c.id === "bar")!)).toEqual(["p1"]);
    expect(moved.find((c) => c.id === "bar")!.tabKey).toBe("t2");
  });
});
