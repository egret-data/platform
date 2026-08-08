import { useEffect } from "react";
import { useDashboardCardData } from "@/api/useSavedChartData";
import type { ChartFilter } from "@/api/charts";

/**
 * Runs a hide-when-empty card's query OUTSIDE the grid and reports whether it came
 * back with rows. Renders nothing.
 *
 * WHY THIS EXISTS AT ALL, because the obvious implementation is a trap. A card's rows
 * are fetched INSIDE its tile (`ChartCardTileView` -> `useDashboardCardData`), but the
 * decision to hide has to be made in `DashboardView`, because that is where
 * `visibleCards` is built and RGL needs the `layout` array and the children to agree.
 * So the naive version — let the tile notice it is empty and tell the parent to drop it —
 * unmounts the very hook that produced the answer: the card disappears, stops querying,
 * and can never learn that it has rows again. It would be hidden until a full reload.
 *
 * Keeping the query mounted independently of the tile is what fixes that. This costs no
 * extra request: `useDashboardCardData` is a react-query `useQuery` keyed on
 * `["dashboardsDbCardRun", ws, dashboardId, cardId, nonce, filters, temporalUnits]`, so a
 * VISIBLE card's probe and its tile share one cache entry and one fetch. The caller must
 * therefore pass byte-identical arguments to both — `DashboardView.cardQueryArgs()` is
 * the single source of them for exactly this reason.
 *
 * Metabase reaches the same place from the other direction: it fetches every dashcard's
 * data into the store up front and then filters ids in `getVisibleCardIds`, so its data
 * never depended on the tile being rendered in the first place.
 */
export function CardEmptinessProbe({
  ws, dashboardId, cardId, refreshNonce, enabled, paramFilters, paramTemporal, onEmpty,
}: {
  ws: string;
  dashboardId: string;
  cardId: string;
  refreshNonce?: number;
  enabled: boolean;
  paramFilters: ChartFilter[];
  paramTemporal: Record<string, string>;
  onEmpty: (cardId: string, empty: boolean) => void;
}) {
  const data = useDashboardCardData(ws, dashboardId, cardId, refreshNonce, enabled, paramFilters, paramTemporal);
  const settled = !data.isLoading;
  // A failed card is NOT hidden — Metabase's shouldHideCard requires
  // `!getDashcardResultsError(...)` too, so an error stays on the board where someone can
  // see it rather than silently removing the tile that would have shown it.
  const empty = !data.error && data.rows.length === 0;

  useEffect(() => {
    // While loading, report nothing: the parent keeps its previous answer, which is what
    // stops a visible card blinking out during a refetch.
    if (!settled) return;
    onEmpty(cardId, empty);
  }, [settled, empty, cardId, onEmpty]);

  return null;
}
