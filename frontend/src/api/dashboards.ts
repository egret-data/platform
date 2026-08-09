import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  dashboardsDbBookmarkAdd,
  dashboardsDbBookmarkRemove,
  dashboardsDbCreate,
  dashboardsDbDuplicate,
  dashboardsDbRemove,
  dashboardsDbRestore,
  dashboardsDbTrash,
  dashboardsDbUpdate,
  getDashboardsDbGetQueryKey,
  getDashboardsDbListQueryKey,
  useDashboardsDbGet,
  useDashboardsDbList,
} from "./generated/dashboards/dashboards";

export interface DashboardCard {
  id: string;
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
  tab_id: string | null;
}
export interface DashboardTab {
  id: string;
  name: string;
  position: number;
}
export interface DashboardSummary {
  id: string; name: string; description: string;
  created_by: string | null; card_count: number; updated_at: string;
  deleted_at?: string | null;
  bookmarked?: boolean;
  folder_id?: string | null;
}
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location" | "temporal_unit";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "does-not-contain" | "starts-with" | "ends-with"        // text / category / location
  | "=" | "!=" | "between" | ">=" | "<="                              // number
  | "range" | "relative" | "single" | "month-year" | "quarter-year" | "all-options";  // date
export type ParamWidget = "dropdown" | "search" | "input";
export interface ParamValuesSource {
  type: "distinct" | "static" | "card";
  static?: string[];
  cardId?: string;
  column?: string;
}
export interface DashboardParameter {
  id: string;
  name: string;
  slug: string;
  type: ParamType;
  operator: ParamOperator;
  default?: unknown;
  required?: boolean;
  widget?: ParamWidget;            // how the value is picked in view mode
  /** "People can pick": true = Multiple values, false = A single value. Absent means the type's
   *  default (see `components/chart/dashboard/multiSelect.ts`). The server stores `parameters` as
   *  an opaque JSONB value, so this needs no backend change. */
  isMultiSelect?: boolean;
  filteringParameters?: string[];  // ids of params whose value constrains THIS param's option list
  valuesSource?: ParamValuesSource;
}

export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; updated_at: string;
  parameters: DashboardParameter[];
  tabs: DashboardTab[];
  width: "fixed" | "full";
  cards: DashboardCard[];
  deleted_at?: string | null;
  bookmarked?: boolean;
}
export interface CardInput {
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
  tab_index: number | null;
}

/** Back-compat query-key helpers; delegate to the generated key factories. */
export const dashboardKeys = {
  list: (ws: string) => getDashboardsDbListQueryKey(ws),
  one: (ws: string, id: string) => getDashboardsDbGetQueryKey(ws, id),
};

export function useDashboards(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useDashboardsDbList<DashboardSummary[], ApiError>(ws, trashed ? { trashed: true } : undefined, {
    query: { select: (r) => r.data as DashboardSummary[] },
  });
}

export function useDashboard(ws: string, id: string | null) {
  return useDashboardsDbGet<DashboardDetail, ApiError>(ws, id ?? "", {
    query: { enabled: !!id, select: (r) => r.data as DashboardDetail },
  });
}

export function useDashboardMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = (id?: string) => {
    qc.invalidateQueries({ queryKey: getDashboardsDbListQueryKey(ws) });
    if (id) qc.invalidateQueries({ queryKey: getDashboardsDbGetQueryKey(ws, id) });
  };
  const create = useMutation<DashboardDetail, ApiError, { name: string; description?: string }>({
    mutationFn: async (b) => (await dashboardsDbCreate(ws, b)).data as DashboardDetail,
    onSuccess: () => invalidate(),
  });
  const update = useMutation<DashboardDetail, ApiError, { id: string; name: string; description?: string; parameters: DashboardParameter[]; tabs: { name: string; position: number }[]; width: "fixed" | "full"; cards: CardInput[] }>({
    mutationFn: async ({ id, ...b }) => (await dashboardsDbUpdate(ws, id, b)).data as DashboardDetail,
    onSuccess: (d) => invalidate(d.id),
  });
  const remove = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await dashboardsDbRemove(ws, id); },
    onSuccess: () => invalidate(),
  });
  const duplicate = useMutation<DashboardDetail, ApiError, string>({
    mutationFn: async (id) => (await dashboardsDbDuplicate(ws, id)).data as DashboardDetail,
    onSuccess: () => invalidate(),
  });
  const trash = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await dashboardsDbTrash(ws, id); },
    onSuccess: () => invalidate(),
  });
  const restore = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await dashboardsDbRestore(ws, id); },
    onSuccess: () => invalidate(),
  });
  const addBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await dashboardsDbBookmarkAdd(ws, id); },
    onSuccess: () => invalidate(),
  });
  const removeBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await dashboardsDbBookmarkRemove(ws, id); },
    onSuccess: () => invalidate(),
  });
  return { create, update, remove, duplicate, trash, restore, addBookmark, removeBookmark };
}
