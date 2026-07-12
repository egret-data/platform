import { useQuery } from "@tanstack/react-query";
import { chartsChRun } from "@/api/generated/charts/charts";
import { dashboardsDbCardRun } from "@/api/generated/dashboards/dashboards";
import type { ChartRunResponse, CardRunBody } from "@/api/generated/model";
import type { ChartFilter } from "@/api/charts";
import type { ChartData } from "@/api/useChartData";

/**
 * Object-scoped saved-object data hooks.
 *
 * These call the new object-scoped endpoints that run ONLY the persisted query
 * of the saved object (no client-supplied spec), so an object guest with a
 * `chart:read` / `dashboard:read` grant can view the shared data without any
 * `query:run` workspace permission. Both map the `ChartRunResponse`
 * `{ columns, rows }` envelope onto the same `ChartData` shape the editor path
 * (`useChartData`) returns, so `ChartRenderer` consumes them identically.
 *
 * A changed `nonce` mints a fresh query key (cache-busted reload); `enabled`
 * gates the request the same way the editor path does.
 */

/** Run a SAVED chart's persisted query via `POST /charts/{id}/data`. */
export function useSavedChartData(ws: string, chartId: string | null, nonce?: number, enabled = true): ChartData {
  const q = useQuery({
    queryKey: ["chartsChRun", ws, chartId, nonce],
    // apiFetch throws on non-2xx, so a resolved envelope is always the success body.
    queryFn: () => chartsChRun(ws, chartId ?? "").then((r) => r.data as ChartRunResponse),
    enabled: enabled && !!chartId,
  });
  return {
    columns: q.data?.columns ?? [],
    rows: (q.data?.rows ?? []) as unknown[][],
    isLoading: q.isLoading,
    error: q.error ? String(q.error) : null,
  };
}

/**
 * Run a dashboard card's persisted chart query via
 * `POST /dashboards/{id}/cards/{card_id}/data`. Only call this for chart-backed
 * cards — a virtual card (text/heading/link, NULL chart_id) returns 400.
 *
 * `filters` carries the resolved dashboard-parameter filters for this card (from
 * `paramFiltersForCard`); the backend validates each `col` against the dataset's
 * real columns and merges them into the persisted query. `temporalUnits` carries the
 * resolved `temporal_unit` param overrides (from `temporalOverridesForCard`) — a
 * `column -> unit token` map the backend validates against the `TemporalUnit` enum and
 * merges to re-grid the persisted query. Both are part of the query key, so changing a
 * dashboard parameter refetches the card.
 */
export function useDashboardCardData(ws: string, dashboardId: string | null, cardId: string | null, nonce?: number, enabled = true, filters: ChartFilter[] = [], temporalUnits: Record<string, string> = {}): ChartData {
  const q = useQuery({
    queryKey: ["dashboardsDbCardRun", ws, dashboardId, cardId, nonce, filters, temporalUnits],
    // apiFetch throws on non-2xx, so a resolved envelope is always the success body.
    queryFn: () => dashboardsDbCardRun(ws, dashboardId ?? "", cardId ?? "", { filters: filters as CardRunBody["filters"], temporal_units: temporalUnits }).then((r) => r.data as ChartRunResponse),
    enabled: enabled && !!dashboardId && !!cardId,
  });
  return {
    columns: q.data?.columns ?? [],
    rows: (q.data?.rows ?? []) as unknown[][],
    isLoading: q.isLoading,
    error: q.error ? String(q.error) : null,
  };
}
