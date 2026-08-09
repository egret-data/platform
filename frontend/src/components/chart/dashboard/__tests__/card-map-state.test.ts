import { describe, it, expect } from "vitest";
import {
  cardMapState, columnsForParam,
  HEADER_COLUMN_TO_FILTER_ON, HEADER_CONNECT_TO, HINT_HEADING_CARD, HINT_LINK_CARD,
  HINT_TEXT_CARD, NO_VALID_FIELDS, OTHER_TAB,
} from "../card-map-state";

const chartCard = {
  cardType: "chart", paramType: "category" as const,
  columns: ["country", "city"], rows: 4,
};

describe("columnsForParam", () => {
  const spec = { dimensions: ["created_at", "country"], columns: ["city"], temporal_units: { created_at: "month" } };
  it("offers dimensions and columns for an ordinary parameter", () => {
    expect(columnsForParam(spec, "category")).toEqual(["created_at", "country", "city"]);
  });
  it("offers only the bucketed columns for a temporal_unit parameter", () => {
    // A "Time grouping" parameter re-grids a column the chart ALREADY buckets; offering it a
    // column with no bucket would produce a mapping the query compiler cannot honour.
    expect(columnsForParam(spec, "temporal_unit")).toEqual(["created_at"]);
    expect(columnsForParam({ dimensions: ["a"], columns: [] }, "temporal_unit")).toEqual([]);
  });
});

describe("cardMapState", () => {
  it("offers the picker with Metabase's header on a chart card", () => {
    expect(cardMapState(chartCard)).toEqual({
      kind: "pick", header: HEADER_COLUMN_TO_FILTER_ON, columns: ["country", "city"],
    });
  });

  it("uses 'Connect to' for a temporal_unit parameter", () => {
    // DashCardCardParameterMapperContent.tsx:111-113.
    const s = cardMapState({ ...chartCard, paramType: "temporal_unit", columns: ["created_at"] });
    expect(s).toMatchObject({ kind: "pick", header: HEADER_CONNECT_TO });
  });

  it("drops the header on a card two rows tall or shorter", () => {
    // DashCardCardParameterMapperContent.tsx:107-109 — there is nowhere to put it.
    expect(cardMapState({ ...chartCard, rows: 2 })).toMatchObject({ kind: "pick", header: null });
    expect(cardMapState({ ...chartCard, rows: 3 })).toMatchObject({ kind: "pick", header: HEADER_COLUMN_TO_FILTER_ON });
  });

  it("says why instead of offering an empty select when nothing can be mapped", () => {
    // This is the live-measured defect: a temporal_unit parameter on a chart with no bucketed
    // column rendered a select whose only option was the placeholder.
    expect(cardMapState({ ...chartCard, columns: [] })).toEqual({ kind: "no-fields", text: NO_VALID_FIELDS });
  });

  it("hints on text and heading cards, and tells the truth about link cards", () => {
    expect(cardMapState({ ...chartCard, cardType: "text" })).toEqual({ kind: "hint", text: HINT_TEXT_CARD });
    // egret interpolates heading text too (DashboardCardView.tsx:104), which Metabase does not —
    // its map gives `heading: ""`. Following egret's behaviour, not Metabase's string table.
    expect(cardMapState({ ...chartCard, cardType: "heading" })).toEqual({ kind: "hint", text: HINT_HEADING_CARD });
    // LinkCard.tsx:8,18 uses the raw url and label with no interpolateParams call, so Metabase's
    // "you can connect widgets to {{variables}} in link cards" would be false here.
    expect(cardMapState({ ...chartCard, cardType: "link" })).toEqual({ kind: "hint", text: HINT_LINK_CARD });
  });

  it("renders nothing on a placeholder card or a chart whose chart is missing", () => {
    expect(cardMapState({ ...chartCard, cardType: "placeholder" })).toEqual({ kind: "none" });
    expect(cardMapState({ ...chartCard, chartMissing: true, columns: [] })).toEqual({ kind: "none" });
  });

  it("has an other-tab branch that is INERT until W7 supplies a filterTabKey", () => {
    // egret parameters are dashboard-level and have no owning card or tab (api/dashboards.ts:54-65),
    // so the route passes `undefined` and this cannot fire today. It is built and pinned here so
    // that W7 (inline parameters) activates it by passing a value, not by redesigning the overlay.
    expect(cardMapState({ ...chartCard, filterTabKey: undefined, cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
    expect(cardMapState({ ...chartCard, filterTabKey: null, cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
    expect(cardMapState({ ...chartCard, filterTabKey: "tab-1", cardTabKey: "tab-0" }))
      .toEqual({ kind: "other-tab", text: OTHER_TAB });
    expect(cardMapState({ ...chartCard, filterTabKey: "tab-0", cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
  });

  it("puts the virtual-card hints ahead of the other-tab check, as Metabase does", () => {
    // DashCardCardParameterMapperContent.tsx checks isVirtual&&isDisabled (146) before
    // isInlineParameterFromAnotherTab (188), so a text card says what it is, not where the filter is.
    expect(cardMapState({ ...chartCard, cardType: "text", filterTabKey: "tab-1", cardTabKey: "tab-0" }))
      .toEqual({ kind: "hint", text: HINT_TEXT_CARD });
  });
});
