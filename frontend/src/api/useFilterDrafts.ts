import { useState } from "react";
import { countChangedFilters } from "@/components/chart/dashboard/filterDrafts";
import type { DashboardParamsState } from "./useDashboardParams";

export interface FilterDraftsState {
  /** What the CARDS run on. Never moves while auto-apply is off and a draft is pending. */
  appliedValues: Record<string, unknown>;
  /** What the FILTER BAR shows — applied values with the draft laid over them. */
  editorValues: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
  changedCount: number;
  apply: () => void;
  cancel: () => void;
  autoApply: boolean;
  setAutoApply: (v: boolean) => void;
}

/** Per-user, per-dashboard, in this browser.
 *
 *  Metabase stores `auto_apply_filters` as a column on the dashboard, shared by every viewer.
 *  egret's dashboard PUT has no field for it and the `dashboards` table has no settings column
 *  (`UpdateDashboard` is name/description/parameters/width/tabs/cards), so a shared setting means
 *  a migration. Rather than smuggle a column into a frontend-only workstream — or hide a scalar
 *  inside the `parameters` array, which is typed as a list of filters and mapped over in eight
 *  places — this is a browser preference, and the switch is labelled as one. */
const key = (dashboardId: string) => `egret:dash-auto-apply:${dashboardId}`;

function readPreference(dashboardId: string): boolean {
  try { return localStorage.getItem(key(dashboardId)) !== "0"; } catch { return true; }
}

export function useFilterDrafts(
  dashboardId: string, params: DashboardParamsState,
): FilterDraftsState {
  const [autoApply, setAutoApplyState] = useState(() => readPreference(dashboardId));
  // null = nothing drafted. An empty object is a different state: a draft that has been emptied.
  const [draft, setDraft] = useState<Record<string, unknown> | null>(null);

  const editorValues = draft === null ? params.values : { ...params.values, ...draft };

  const setValue = (slug: string, value: unknown) => {
    if (autoApply) { params.setValue(slug, value); return; }
    setDraft((prev) => ({ ...(prev ?? {}), [slug]: value }));
  };

  const apply = () => {
    if (draft) params.setValues(draft);   // ONE URL write — see setValues
    setDraft(null);
  };

  const cancel = () => setDraft(null);

  const setAutoApply = (v: boolean) => {
    try { localStorage.setItem(key(dashboardId), v ? "1" : "0"); } catch { /* private mode */ }
    setAutoApplyState(v);
    // Switching auto-apply on removes the only control that could commit a pending draft, so
    // commit it here rather than stranding a change the user can no longer reach.
    if (v && draft) { params.setValues(draft); setDraft(null); }
  };

  return {
    appliedValues: params.values,
    editorValues,
    setValue,
    changedCount: countChangedFilters(params.values, editorValues),
    apply,
    cancel,
    autoApply,
    setAutoApply,
  };
}
