import { useMutation, useQueryClient } from "@tanstack/react-query";

import type { ApiError } from "./client";

import {
  alertsBookmarkAdd,
  alertsBookmarkRemove,
  alertsCreate,
  alertsRemove,
  alertsRestore,
  alertsSetFolder,
  alertsTrash,
  alertsRunNow,
  alertsSetActive,
  alertsUpdate,
  getAlertsDetailQueryKey,
  getAlertsListQueryKey,
  getAlertsRunsQueryKey,
  useAlertsDetail,
  useAlertsList,
  useAlertsRuns,
} from "./generated/alerts/alerts";
import type {
  CreateSchedule,
  RecipientDto,
  RunDto,
  ScheduleDetailDto,
  ScheduleSummaryDto,
  UpdateSchedule,
} from "./generated/model";

export type ScheduleType = "alert" | "report" | "dataapi_alert";
export type RunState = "success" | "working" | "error" | "noop" | "grace";

export type ScheduleSummary = ScheduleSummaryDto;
export type ScheduleDetail = ScheduleDetailDto;
export type ScheduleRun = RunDto;
export type Recipient = RecipientDto;

/**
 * Schedules in a workspace, optionally narrowed to one tab.
 *
 * `type` is sent to the SERVER rather than filtered in the browser: the two
 * tabs are separate namespaces (a name may be reused across them), so filtering
 * client-side would fetch rows the tab will never show and make the counts
 * disagree with what the server considers a duplicate.
 */
export function useSchedules(ws: string, type?: ScheduleType, trashed = false) {
  // `select` unwraps orval's `{data, headers}` envelope, matching how every
  // other api module here exposes a list — without it every consumer would
  // have to reach through `.data`.
  return useAlertsList<ScheduleSummary[], ApiError>(
    ws,
    { ...(type ? { type } : {}), ...(trashed ? { trashed: true } : {}) },
    {
      query: { enabled: Boolean(ws), select: (r) => r.data as ScheduleSummary[] },
    },
  );
}

export function useSchedule(ws: string, id: string | null) {
  return useAlertsDetail<ScheduleDetail, ApiError>(ws, id ?? "", {
    query: {
      enabled: Boolean(ws && id),
      select: (r) => r.data as ScheduleDetail,
    },
  });
}

/**
 * The run log for one schedule. Only fetched while the drawer is open — it is
 * the answer to "why didn't I get an email?", not something every list render
 * needs.
 */
export function useScheduleRuns(ws: string, id: string | null) {
  return useAlertsRuns<ScheduleRun[], ApiError>(ws, id ?? "", {
    query: {
      enabled: Boolean(ws && id),
      select: (r) => r.data as ScheduleRun[],
    },
  });
}

/**
 * Create/update/delete, each invalidating the list so the state badge and the
 * next-run column reflect the write without a manual refresh.
 */
export function useScheduleMutations(ws: string) {
  const qc = useQueryClient();
  // Each tab is its own query key (`[url, {type}]`), so invalidating one
  // variant would leave the other tab stale. Called with no params the key is
  // just `[url]`, which react-query treats as a PREFIX — so this invalidates
  // every tab at once.
  const invalidate = () =>
    qc.invalidateQueries({ queryKey: getAlertsListQueryKey(ws) });

  const create = useMutation({
    mutationFn: (body: CreateSchedule) => alertsCreate(ws, body),
    onSuccess: invalidate,
  });
  const update = useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateSchedule }) =>
      alertsUpdate(ws, id, body),
    onSuccess: invalidate,
  });
  const remove = useMutation({
    mutationFn: (id: string) => alertsRemove(ws, id),
    onSuccess: invalidate,
  });
  /**
   * "Run now" (P5). The run writes its outcome onto the schedule's
   * `last_state` (KI-032; only `next_run_at` stays as it was), so the list's
   * Last run and the detail badge change as well as the run log — all three
   * are invalidated.
   *
   * Unwraps orval's `{data, status}` envelope to the `RunDto` itself — orval
   * widens `data` to `RunDto | void` because the generated type also covers
   * the 404 response, but `apiFetch` throws on non-2xx, so a resolved promise
   * here was always the 200 body.
   */
  const runNow = useMutation({
    mutationFn: async (id: string) => {
      const res = await alertsRunNow(ws, id);
      return res.data as RunDto;
    },
    onSuccess: (_run, id) =>
      Promise.all([
        qc.invalidateQueries({ queryKey: getAlertsRunsQueryKey(ws, id) }),
        qc.invalidateQueries({ queryKey: getAlertsDetailQueryKey(ws, id) }),
        invalidate(),
      ]),
  });
  const setActive = useMutation({
    mutationFn: ({ id, active }: { id: string; active: boolean }) =>
      alertsSetActive(ws, id, { active }),
    onSuccess: invalidate,
  });
  /** Soft delete. `remove` is the permanent one, offered only from the trash. */
  const trash = useMutation({
    mutationFn: (id: string) => alertsTrash(ws, id),
    onSuccess: invalidate,
  });
  const restore = useMutation({
    mutationFn: (id: string) => alertsRestore(ws, id),
    onSuccess: invalidate,
  });
  const setFolder = useMutation({
    mutationFn: ({ id, folder_id }: { id: string; folder_id: string | null }) =>
      alertsSetFolder(ws, id, { folder_id }),
    onSuccess: invalidate,
  });
  const addBookmark = useMutation({
    mutationFn: (id: string) => alertsBookmarkAdd(ws, id),
    onSuccess: invalidate,
  });
  const removeBookmark = useMutation({
    mutationFn: (id: string) => alertsBookmarkRemove(ws, id),
    onSuccess: invalidate,
  });
  return {
    create,
    update,
    remove,
    runNow,
    setActive,
    trash,
    restore,
    setFolder,
    addBookmark,
    removeBookmark,
  };
}
