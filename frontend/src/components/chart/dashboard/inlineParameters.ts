import type { DashboardParameter } from "@/api/dashboards";

/**
 * INLINE PARAMETERS — a filter drawn on a card instead of on the top bar.
 *
 * THE FIELD IS `card.options.inlineParameters: string[]`, a list of PARAMETER ids hosted by that
 * card. It is Metabase's shape (`dashcard.inline_parameters`, dashboard/utils.ts:128), and it goes
 * on the card rather than on the parameter for a reason specific to egret: `serializeLayout`
 * (tabs.ts:34-48) sends cards WITHOUT ids and `replace_layout`
 * (crates/egret-bi/src/dashboard.rs:344-376) deletes and re-inserts every row, so a card id does
 * not survive a save. A `parameter.cardId` would dangle the first time the dashboard was saved.
 * Parameter ids do survive: `parameters` is one JSONB column written and read verbatim.
 *
 * NO BACKEND CHANGE. `CardInput.options` is `serde_json::Value` with no validation
 * (crates/egret-analytics-api/src/bi.rs:393-394), bound straight into the `options` jsonb column
 * and echoed back by `DashboardCardDto.options` (bi.rs:349) — the same argument `options.hideEmpty`
 * made in hide-empty.ts:6-9.
 *
 * AN INLINE PARAMETER STILL FILTERS EVERY CARD IT IS MAPPED TO. Only the WIDGET moves. Metabase
 * subtracts inline parameters from the header list alone (selectors.ts:447-454) and leaves the
 * parameter list that feeds values and mappings untouched; egret's `paramFiltersForCard` likewise
 * reads the full `parameters` array. `topBarParameters` must therefore reach exactly two call
 * sites — the view-mode filter bar and the edit-mode chip row.
 */

/** The minimum a card has to look like for this module. Structural, not `DraftCard`, so the tests
 *  never have to build a dashboard. */
export interface InlineHostCard {
  id: string;
  card_type: string;
  options: Record<string, unknown>;
  tabKey?: string | null;
}

/** Metabase's `supportsInlineParameters` (utils.ts:118-122): the question card and the heading
 *  card. egret's question card is `chart`. A text card interpolates {{slug}} in its own body, a
 *  link card is one anchor and a placeholder has no content — none of them has a header row a
 *  filter widget could sit in, which is the same line Metabase draws. */
export function supportsInlineParameters(cardType: string): boolean {
  return cardType === "chart" || cardType === "heading";
}

/** The ids a card claims. Defensive about the shape because `options` is an untyped JSON bag that
 *  a hand-edited dashboard row can put anything into. */
export function inlineParameterIds(card: { options: Record<string, unknown> }): string[] {
  const raw = card.options.inlineParameters;
  return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : [];
}

/** Which card hosts `paramId`, across the WHOLE dashboard — not the active tab. Metabase's
 *  `findDashCardForInlineParameter` (utils.ts:140-149) composed with `getCurrentDashcards`
 *  (selectors.ts:223-226), which is every dashcard. A per-tab lookup would make a filter appear
 *  and disappear as the viewer switched tabs. */
export function findHostCardId(paramId: string, cards: readonly InlineHostCard[]): string | null {
  return cards.find((c) => inlineParameterIds(c).includes(paramId))?.id ?? null;
}

/**
 * The tab of the card hosting `paramId`, or `undefined` when no card hosts it.
 *
 * THIS IS W2'S ACTIVATION POINT. `cardMapState` (card-map-state.ts:84-99) already has an
 * `other-tab` branch — "The selected filter is on another tab." — that W2 built, tested and left
 * inert by passing `filterTabKey: undefined`, because a dashboard-level filter has no owning card
 * and therefore no tab. An inline parameter finally gives it one.
 *
 * `undefined` and `null` are DIFFERENT ANSWERS and the distinction is what keeps the branch honest.
 * `cardMapState` guards with `filterTabKey != null`, so `undefined` (no host at all) leaves the
 * hint inert exactly as it is today — and so does `null`, which is a real host on a dashboard that
 * has no tabs. Only a genuine tab key can fire it.
 */
export function hostTabKey(
  paramId: string,
  cards: readonly InlineHostCard[],
): string | null | undefined {
  const host = cards.find((c) => inlineParameterIds(c).includes(paramId));
  return host ? (host.tabKey ?? null) : undefined;
}

/** The parameters this card draws, in the CARD's order — the card's list is the authored order,
 *  the dashboard's array is the top bar's. An id with no matching parameter is skipped rather than
 *  rendered as a hole. */
export function inlineParametersFor(
  card: InlineHostCard,
  parameters: readonly DashboardParameter[],
): DashboardParameter[] {
  const byId = new Map(parameters.map((p) => [p.id, p]));
  return inlineParameterIds(card).map((id) => byId.get(id)).filter((p): p is DashboardParameter => !!p);
}

/** The parameters the top bar draws: everything no card has claimed. */
export function topBarParameters(
  parameters: readonly DashboardParameter[],
  cards: readonly InlineHostCard[],
): DashboardParameter[] {
  const hosted = new Set(cards.flatMap((c) => inlineParameterIds(c)));
  return parameters.filter((p) => !hosted.has(p.id));
}

/** Fold a reordered TOP-BAR list back into the full parameter array.
 *
 *  W6's drag-reorder hands its `onReorder` the whole list it was given and the route writes it
 *  straight into the draft (`onReorder={setDraftParameters}`). Once the top bar is a SUBSET,
 *  writing it back unmerged would delete every inline parameter. The hosted ones are appended,
 *  which is lossless: their position in this array never renders anything — a card draws them in
 *  its own `inlineParameters` order. */
export function mergeTopBarOrder(
  parameters: readonly DashboardParameter[],
  orderedTopBar: readonly DashboardParameter[],
): DashboardParameter[] {
  const seen = new Set(orderedTopBar.map((p) => p.id));
  return [...orderedTopBar, ...parameters.filter((p) => !seen.has(p.id))];
}

/** Move `paramId` to `destCardId`, or to the top bar when `destCardId` is null. Removes it from
 *  whichever card held it first, so it can never be on two cards at once.
 *
 *  A destination that cannot host is refused by returning the SAME array reference, so the caller
 *  writes no draft. Accepting it would take the filter off the top bar and give it nowhere to
 *  draw. */
export function moveParameterToCard<T extends InlineHostCard>(
  cards: readonly T[],
  paramId: string,
  destCardId: string | null,
): T[] {
  if (destCardId !== null) {
    const dest = cards.find((c) => c.id === destCardId);
    if (!dest || !supportsInlineParameters(dest.card_type)) return cards as T[];
  }
  return cards.map((c) => {
    const held = inlineParameterIds(c);
    const shouldHold = c.id === destCardId;
    const holds = held.includes(paramId);
    if (holds === shouldHold) return c;
    const next = shouldHold ? [...held, paramId] : held.filter((id) => id !== paramId);
    return { ...c, options: { ...c.options, inlineParameters: next } };
  });
}

/** Delete parameters and every reference to them — Metabase's `removeParameterAndReferences`
 *  (parameters.tsx:357-376), which also strips the ids out of other parameters'
 *  `filteringParameters` so a linked filter is not left pointing at nothing. Same reference back
 *  when nothing is dropped, so the caller can skip the draft write. */
export function dropParameters(
  parameters: readonly DashboardParameter[],
  ids: ReadonlySet<string>,
): DashboardParameter[] {
  if (ids.size === 0) return parameters as DashboardParameter[];
  return parameters
    .filter((p) => !ids.has(p.id))
    .map((p) =>
      p.filteringParameters?.some((id) => ids.has(id))
        ? { ...p, filteringParameters: p.filteringParameters.filter((id) => !ids.has(id)) }
        : p,
    );
}

/** The only key of a replaced card's `options` that survives. `replaceCard` resets `options` to
 *  `{}` and §9 of the design records that as NOT a divergence — but the filter list is a
 *  PLACEMENT, not a property of the chart, and Metabase's own replaceCard leaves
 *  `inline_parameters` alone (cards-typed.ts:262-270). Without this, replacing a chart teleports
 *  its filter back to the top bar. */
export function keepInlineParameters(options: Record<string, unknown>): Record<string, unknown> {
  const ids = inlineParameterIds({ options });
  return ids.length > 0 ? { inlineParameters: ids } : {};
}

/** The inverse, for a CLONE. Two cards claiming one parameter id is not a state this model can
 *  render — `findHostCardId` picks the first and the clone draws nothing — so a duplicated card
 *  starts with no filters and the original keeps them.
 *
 *  DELIBERATE DIVERGENCE, called out in the PR body: Metabase duplicates the parameters as well,
 *  minting fresh ids and remapping `parameter_mappings` (cards-typed.ts:416-441). Doing that here
 *  would fork the URL contract (a new slug appears in the query string) and belongs in its own
 *  change; stripping is one line and cannot be wrong. */
export function withoutInlineParameters(options: Record<string, unknown>): Record<string, unknown> {
  if (!("inlineParameters" in options)) return options;
  const { inlineParameters: _dropped, ...rest } = options;
  return rest;
}

/** Metabase's `getDashcardTitle` (MoveParameterMenu.tsx:136-146), in egret's terms: a heading is
 *  named by its own text, a chart by its title override, then by the chart's name. */
export function destinationLabel(
  card: { card_type: string; options: Record<string, unknown> },
  chartName: string | undefined,
): string {
  const str = (v: unknown) => (typeof v === "string" ? v.trim() : "");
  if (card.card_type === "heading") return str(card.options.text) || "Heading";
  return str(card.options.title) || (chartName ?? "").trim() || "Card";
}

export interface MoveDestination {
  cardId: string;
  label: string;
  /** The tab whose `<optgroup>` this belongs in, or null on a dashboard with no tabs. */
  tabName: string | null;
}

/** Every card a filter could be moved to, grouped the way `MoveParameterMenu` groups them
 *  (`:104-129`): one group per tab that actually has a hostable card, in tab order; and when no
 *  tab has one, a flat ungrouped list. */
export function moveDestinations(args: {
  cards: readonly InlineHostCard[];
  tabs: readonly { key: string; name: string; position: number }[];
  chartNameOf: (cardId: string) => string | undefined;
}): MoveDestination[] {
  const { cards, tabs, chartNameOf } = args;
  const hostable = cards.filter((c) => supportsInlineParameters(c.card_type));
  const label = (c: InlineHostCard) => destinationLabel(c, chartNameOf(c.id));
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const grouped = ordered.flatMap((t) =>
    hostable
      .filter((c) => (c.tabKey ?? null) === t.key)
      .map((c) => ({ cardId: c.id, label: label(c), tabName: t.name })),
  );
  if (grouped.length > 0) return grouped;
  return hostable.map((c) => ({ cardId: c.id, label: label(c), tabName: null }));
}

/** Metabase's buffers. 100 for a question card (DashCardVisualization.tsx:445, "Bigger buffer
 *  space to account for varying chart padding"), and the 24 default for a heading
 *  (use-responsive-parameter-list.ts:13).
 *
 *  On an egret question card the 100 is already spoken for. The hover cluster W5 built is
 *  `absolute right-1` and holds up to TWO `Button size="sm"` glyph buttons — the Edit-question
 *  pencil and the export menu (`DashboardCardView.tsx:77-89`) — at 36px each plus a 2px `gap-0.5`,
 *  so it contributes NO layout width while covering 78px of the row's trailing edge. The header
 *  row reserves that explicitly (see `inlineFilterGutterClass`); the buffer is what is left over
 *  for chart padding. */
export const INLINE_PARAM_BUFFER_PX = 100;
export const INLINE_PARAM_HEADING_BUFFER_PX = 24;

/**
 * The right padding a chart card's header row needs so the hover cluster cannot land on the last
 * filter pill.
 *
 * THE PLAN FOR THIS WORKSTREAM SAID `pr-10` AND THAT WAS WRITTEN AGAINST THE PRE-W5 HEADER, which
 * had one hover button. W5 added the Edit-question pencil beside the export menu
 * (`DashboardCardView.tsx:77-89`), so the cluster is now up to two buttons wide and the old
 * reservation would leave the pencil sitting on top of a pill.
 *
 * The arithmetic, from `ui/button.tsx:66`: `size="sm"` around a single `<svg>` resolves to
 * `h-8 … has-[>svg]:px-2.5`, i.e. 10 + 16 + 10 = 36px. `gap-0.5` between them is 2px, and the
 * cluster sits at `right-1` = 4px. One button → 40px → `pr-10`. Two → 4 + 36 + 2 + 36 = 78px →
 * `pr-20` (80px). Neither button takes any layout width, because the cluster is `absolute`.
 *
 * This is a real collision and it is only visible on hover, so it survives every static review —
 * which is why the class is asserted rather than eyeballed.
 */
export function inlineFilterGutterClass(hoverButtonCount: number): string {
  if (hoverButtonCount >= 2) return "pr-20";
  if (hoverButtonCount === 1) return "pr-10";
  return "pr-4";
}

/** Metabase's collision test, verbatim (use-responsive-parameter-list.ts:29-34):
 *  `reserved + list + buffer > container`.
 *
 *  `reservedWidth` is what SHARES the row — the card title or the heading text — measured at its
 *  intrinsic width, not its laid-out width (see `useCollapsedParameterList`). `listWidth` is the
 *  ALWAYS-EXPANDED copy of the list, because measuring the visible one oscillates. */
export function shouldCollapseParameterList(m: {
  containerWidth: number;
  reservedWidth: number;
  listWidth: number;
  bufferPx: number;
}): boolean {
  // Nothing measured yet — first paint, or jsdom, which reports 0 for everything. Metabase bails
  // to false when a ref is missing; starting collapsed would flash a "Show filters" button on
  // every card on every load.
  if (m.containerWidth <= 0) return false;
  return m.reservedWidth + m.listWidth + m.bufferPx > m.containerWidth;
}
