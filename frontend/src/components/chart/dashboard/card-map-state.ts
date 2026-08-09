import type { ParamType } from "@/api/dashboards";

/** Metabase's header strings (DashCardCardParameterMapperContent.tsx:106-120). `Variable to map to`
 *  is deliberately not ported: it is the header for a NATIVE SQL card with template variables, and
 *  every egret dashboard card is a spec-built chart. */
export const HEADER_COLUMN_TO_FILTER_ON = "Column to filter on";
export const HEADER_CONNECT_TO = "Connect to";

/** Metabase's disabled-mapper tooltip is "This card doesn't have any fields or parameters that can
 *  be mapped to this parameter type." (DashCardCardParameterMapperButton.tsx:92). egret cards carry
 *  no parameters of their own, so that clause is dropped; and this codebase calls them filters, not
 *  parameters, everywhere the user can see. */
export const NO_VALID_FIELDS = "This card doesn't have any fields that can be mapped to this filter.";

/** Metabase says "widgets"; egret's UI says filters, so these match egret's own vocabulary.
 *  Heading gets a hint where Metabase's map gives it `""`, because egret's heading card DOES
 *  interpolate `{{slug}}` (DashboardCardView.tsx:104, same as text at :99). The link string is
 *  NOT Metabase's — `LinkCard.tsx:8,18` uses the raw url and label with no interpolation, so
 *  promising `{{variables}}` there would document a feature egret does not have. */
export const HINT_TEXT_CARD = "You can connect filters to {{variables}} in text cards.";
export const HINT_HEADING_CARD = "You can connect filters to {{variables}} in heading cards.";
export const HINT_LINK_CARD = "Link cards can't use a filter's value.";

/** DashCardCardParameterMapperContent.tsx:192. See `filterTabKey` — inert in egret today. */
export const OTHER_TAB = "The selected filter is on another tab.";

/** What the filter-mapping overlay should show on one card while a filter is selected.
 *  Metabase computes the same disjunction inline inside a component that needs a Redux store, a
 *  Question instance and a dashcard; splitting it out here is what lets it be exhaustively tested
 *  without mounting the 800-line dashboard route. */
export type CardMapState =
  /** The card can take this filter: show the header (unless the card is too short) and the picker. */
  | { kind: "pick"; header: string | null; columns: string[] }
  /** A chart card with nothing on it that this parameter type can attach to. */
  | { kind: "no-fields"; text: string }
  /** A virtual card, which reaches filters some other way — or not at all. */
  | { kind: "hint"; text: string }
  /** The filter belongs to a card on a different tab. Inert until W7; see `filterTabKey`. */
  | { kind: "other-tab"; text: string }
  /** Nothing at all — the overlay does not render. */
  | { kind: "none" };

export interface CardMapInput {
  /** `DashboardCard.card_type`: "chart" | "text" | "heading" | "link" | "placeholder". */
  cardType: string;
  /** The type of the parameter currently selected in the edit sidebar. */
  paramType: ParamType;
  /** The columns this card can offer for that parameter — normally from `columnsForParam`. */
  columns: string[];
  /** The card's height in grid rows (`size_y`). Metabase drops the header at 2 or fewer. */
  rows: number;
  /** True for a chart card whose chart could not be resolved (deleted, or not loaded yet). */
  chartMissing?: boolean;
  /**
   * The tab the selected filter itself lives on, when the filter is pinned to a card.
   *
   * THIS IS INERT TODAY AND DELIBERATELY SO. egret's `DashboardParameter` (api/dashboards.ts:54-65)
   * has no owning card and no tab — every filter is dashboard-level — so `DashboardView` passes
   * `undefined` and this branch cannot fire. It exists, tested, so that W7 (inline parameters)
   * turns it on by passing a value rather than by redesigning the overlay. Do not synthesise a
   * value for it from anything else; there is nothing here that means what Metabase means.
   */
  filterTabKey?: string | null;
  /** The draft tab key of the card being drawn. Only read when `filterTabKey` is set. */
  cardTabKey?: string | null;
}

/** The columns a chart card can offer for one parameter type. A `temporal_unit` parameter re-grids
 *  a column the chart ALREADY buckets, so it can only attach to a key of `spec.temporal_units`;
 *  everything else attaches to any dimension or column. Lifted out of the route (it was inline in
 *  the overlay's JSX) so every input to `cardMapState` is pure. */
export function columnsForParam(
  spec: { dimensions: string[]; columns: string[]; temporal_units?: Record<string, string> },
  paramType: ParamType,
): string[] {
  if (paramType === "temporal_unit") return Object.keys(spec.temporal_units ?? {});
  return [...spec.dimensions, ...spec.columns];
}

/** Decide what the overlay shows. The order of the branches mirrors Metabase's
 *  (DashCardCardParameterMapperContent.tsx:146-209): virtual-card hints first, then the tab check,
 *  then the no-fields case, then the picker. A text card should say what it is, not where the
 *  filter is. */
export function cardMapState(input: CardMapInput): CardMapState {
  const { cardType, paramType, columns, rows, chartMissing, filterTabKey, cardTabKey } = input;

  if (cardType === "placeholder") return { kind: "none" };
  if (cardType === "text") return { kind: "hint", text: HINT_TEXT_CARD };
  if (cardType === "heading") return { kind: "hint", text: HINT_HEADING_CARD };
  if (cardType === "link") return { kind: "hint", text: HINT_LINK_CARD };
  if (cardType !== "chart" || chartMissing) return { kind: "none" };

  if (filterTabKey != null && filterTabKey !== cardTabKey) return { kind: "other-tab", text: OTHER_TAB };
  if (columns.length === 0) return { kind: "no-fields", text: NO_VALID_FIELDS };

  const header = rows <= 2
    ? null
    : paramType === "temporal_unit" ? HEADER_CONNECT_TO : HEADER_COLUMN_TO_FILTER_ON;
  return { kind: "pick", header, columns };
}
