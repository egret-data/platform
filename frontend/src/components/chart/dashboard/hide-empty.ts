import type { DashboardCard } from "@/api/dashboards";

/**
 * Which cards "Hide this card if there are no results" should take off the board.
 *
 * Metabase calls the setting `card.hide_empty` and stores it on the DASHCARD; egret
 * stores it as `options.hideEmpty` on the card, which needs no backend change at all —
 * `options` is `jsonb` end to end (migrations/0015_dashboard.sql:24,
 * DashboardCard.options: Record<string, unknown>).
 *
 * WHAT THIS DELIBERATELY DOES NOT DO — measured against Metabase 3033 rather than
 * assumed, on a scratch dashboard built for the purpose:
 *
 *   Bar | EMPTY | Row at cols 0/8/16, empty card hidden  ->  Bar . . . . . Row
 *   one card at row 0 hidden, another at row 6           ->  the row-6 card drew at row 0
 *
 * i.e. Metabase closes the gap VERTICALLY and leaves the horizontal hole. That is not
 * special-case code on their side, it is just what react-grid-layout does with
 * `compactType="vertical"` when a child disappears. egret's DashboardGridLayout passes
 * the identical `compactType="vertical"`, so simply dropping these ids from
 * `visibleCards` — which feeds BOTH the RGL `layout` array and the children — reproduces
 * Metabase's behaviour exactly and this module needs no layout maths of its own. The
 * brief for this work expected the grid to be the hard part; it turned out to be free,
 * and the real cost was getting `rows.length === 0` out of the tile (see
 * CardEmptinessProbe).
 *
 * `emptiness` is the probe's report: id -> "this card's query returned zero rows".
 */
export function hiddenEmptyCardIds(
  cards: readonly DashboardCard[],
  emptiness: Readonly<Record<string, boolean>>,
  editing: boolean,
  blocked = false,
): Set<string> {
  // A required dashboard parameter with no value blocks EVERY card — they render "Set the
  // required filters to load this card" and never query. Nothing reports, so without this
  // guard the unknown-means-hidden default below would swallow the whole board and leave
  // the user staring at a filter bar with no explanation.
  if (blocked) return new Set();

  // An author editing the board must be able to see and configure a card that is
  // currently empty. Metabase's getVisibleCards has the same guard (it returns every
  // tab card when isEditing). It is also load-bearing here for a second reason: egret's
  // onLayoutChange persists RGL's compacted positions while editing, so a card vanishing
  // mid-edit would silently rewrite its neighbours' saved grid_row.
  if (editing) return new Set();

  const hidden = new Set<string>();
  for (const c of cards) {
    // Only a chart card runs a query, so only it can be empty. Metabase hides the toggle
    // itself for virtual dashcards (`getHidden: ([{ card }]) => isVirtualDashCard(card)`).
    if (c.card_type !== "chart") continue;
    // A chart card whose chart is gone runs no query, so no probe ever reports for it and
    // the default below would hide it forever. Its tile's own "Chart not found" is what
    // should show — the same reasoning that keeps an ERRORED card on the board.
    if (!c.chart_id) continue;
    if (!c.options.hideEmpty) continue;
    // Default TRUE while the answer is unknown. Metabase's shouldHideCard returns
    // `!wasVisible` for a card whose data is still loading, so a card nobody has seen yet
    // starts hidden and appears once it turns out to have rows — rather than drawing and
    // then vanishing. On a REFETCH the probe stops reporting, so the map still holds the
    // previous answer and a visible card stays visible.
    if (emptiness[c.id] ?? true) hidden.add(c.id);
  }
  return hidden;
}
