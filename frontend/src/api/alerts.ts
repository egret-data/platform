import { useMutation, useQueryClient } from "@tanstack/react-query";

import type { ApiError } from "./client";

import {
  alertsCreate,
  alertsRemove,
  alertsUpdate,
  getAlertsListQueryKey,
  useAlertsDetail,
  useAlertsList,
  useAlertsRuns,
} from "./generated/alerts/alerts";
import type {
  CreateSchedule,
  RunDto,
  ScheduleDetailDto,
  ScheduleSummaryDto,
  UpdateSchedule,
} from "./generated/model";

export type ScheduleType = "alert" | "report";
export type RunState = "success" | "working" | "error" | "noop" | "grace";

export type ScheduleSummary = ScheduleSummaryDto;
export type ScheduleDetail = ScheduleDetailDto;
export type ScheduleRun = RunDto;

/**
 * Schedules in a workspace, optionally narrowed to one tab.
 *
 * `type` is sent to the SERVER rather than filtered in the browser: the two
 * tabs are separate namespaces (a name may be reused across them), so filtering
 * client-side would fetch rows the tab will never show and make the counts
 * disagree with what the server considers a duplicate.
 */
export function useSchedules(ws: string, type?: ScheduleType) {
  // `select` unwraps orval's `{data, headers}` envelope, matching how every
  // other api module here exposes a list — without it every consumer would
  // have to reach through `.data`.
  return useAlertsList<ScheduleSummary[], ApiError>(ws, type ? { type } : undefined, {
    query: { enabled: Boolean(ws), select: (r) => r.data as ScheduleSummary[] },
  });
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
  return { create, update, remove };
}
