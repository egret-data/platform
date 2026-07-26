import type { Chart } from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";

export interface ChartData {
  columns: { name: string; type: string }[];
  rows: unknown[][];
  isLoading: boolean;
  error: string | null;
}

/**
 * Run a saved chart's query for the dashboard EDIT-mode preview.
 *
 * View mode uses `useDashboardCardData` (`@/api/useSavedChartData`), which is
 * object-scoped so share recipients keep working. This path is
 * workspace-scoped and now runs through `POST /charts/run` rather than the
 * free-form SQL endpoint.
 */
export function useChartData(ws: string, chart: Chart | null, nonce?: number, enabled = true): ChartData {
  return useSpecData(ws, chart?.dataset_id ?? null, chart?.params.spec ?? null, enabled && !!chart, nonce);
}
