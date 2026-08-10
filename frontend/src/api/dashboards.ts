import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  dashboardsDbBookmarkAdd,
  dashboardsDbBookmarkRemove,
  dashboardsDbCreate,
  dashboardsDbDuplicate,
  dashboardsDbRemove,
  dashboardsDbRestore,
  dashboardsDbRevisionRevert,
  dashboardsDbSetCertification,
  dashboardsDbTrash,
  dashboardsDbUpdate,
  getDashboardsDbGetQueryKey,
  getDashboardsDbListQueryKey,
  getDashboardsDbRevisionsQueryKey,
  useDashboardsDbGet,
  useDashboardsDbList,
  useDashboardsDbRevisions,
} from "./generated/dashboards/dashboards";

export interface DashboardCard {
  id: string;
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  /** Untyped per-card bag, stored as `jsonb` and echoed back verbatim (bi.rs:349,393-394).
   *  Live keys: `title`, `description`, `text`, `url`, `label`, `vizOverride`, `paramMappings`,
   *  `autoHeight`, `hideEmpty`, `noBackground`, `clickBehavior`, and `inlineParameters` — the ids
   *  of the dashboard parameters this card draws instead of the top bar (see
   *  `components/chart/dashboard/inlineParameters.ts`). */
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
  created_by: string | null; created_by_name: string | null;
  created_at: string | null;
  updated_by: string | null; updated_by_name: string | null;
  updated_at: string;
  folder_id?: string | null;
  parameters: DashboardParameter[];
  tabs: DashboardTab[];
  width: "fixed" | "full";
  cards: DashboardCard[];
  /** Certified by a steward (migration 0108). Written only through `setCertification`, never
   *  through the ordinary save — see the endpoint's own note for why that separation matters. */
  certified?: boolean;
  /** The certifier's email, empty while `certified` is false. */
  certified_by?: string;
  certification_details?: string;
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
  const revert = useMutation<DashboardDetail, ApiError, { id: string; revisionId: string }>({
    mutationFn: async ({ id, revisionId }) =>
      (await dashboardsDbRevisionRevert(ws, id, revisionId)).data as DashboardDetail,
    // A revert changes the dashboard AND appends to its history, so both caches are stale.
    onSuccess: (d) => {
      invalidate(d.id);
      qc.invalidateQueries({ queryKey: getDashboardsDbRevisionsQueryKey(ws, d.id) });
    },
  });
  /** Certify or withdraw certification. A SEPARATE endpoint from `update` because it is guarded
   *  by the platform-scoped `dashboard:certify` — an editor saving the dashboard cannot set it. */
  const setCertification = useMutation<DashboardDetail, ApiError, { id: string; certified: boolean; details?: string }>({
    mutationFn: async ({ id, certified, details }) =>
      (await dashboardsDbSetCertification(ws, id, { certified, details: details ?? "" })).data as DashboardDetail,
    onSuccess: (d) => invalidate(d.id),
  });
  return { create, update, remove, duplicate, trash, restore, addBookmark, removeBookmark, revert, setCertification };
}

export interface DashboardRevision {
  id: string;
  kind: "baseline" | "edit" | "revert";
  created_at: string;
  created_by: string | null;
  created_by_name: string | null;
  is_current: boolean;
}

/** A dashboard's history, newest first. `enabled` is false until the History tab is open, so an
 *  info sidebar opened on Overview costs nothing. */
export function useDashboardRevisions(ws: string, id: string | null, enabled: boolean) {
  return useDashboardsDbRevisions<DashboardRevision[], ApiError>(ws, id ?? "", {
    query: { enabled: enabled && !!id, select: (r) => r.data as DashboardRevision[] },
  });
}
