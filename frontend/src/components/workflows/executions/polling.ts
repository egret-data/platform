import { useEffect, useState } from "react";
import type { workflowGetExecutionResponse } from "@/api/generated/workflow/workflow";
import { isTerminalState } from "@/components/workflows/executionState";
import { fullTimestamp } from "@/lib/relativeTime";

/**
 * HOW THE EXECUTION PAGES FOLLOW A RUN (W7b spec §5.1, §9). egret's APIs are
 * poll-only, so a page reads again on a timer until what it shows can no
 * longer change. Generalised from W7a's `ExecutionMinimal.tsx`.
 */

/** How often a live query is read again. */
export const POLL_MS = 2000;
/** The longest wait between reads while they keep failing. */
export const POLL_MAX_MS = 30_000;

/** What `nextPollMs` reads of a react-query state: `query.state` fits. */
export interface PollState<T> {
  data: T | undefined;
  status: string;
  dataUpdatedAt: number;
}

/**
 * When to read again, for any query: never once `finished(data)` says the
 * answer is final. While reads fail, the wait grows with the time since the
 * last good answer -- 2 s, then 4, 8, 16, capped at 30 -- so a blip costs one
 * late refresh and an outage is not hammered. It is computed from timestamps
 * react-query keeps, because its own failure count restarts with every fetch.
 *
 * With nothing ever loaded and the read failed, it stops: the page's full
 * error and its Retry take over.
 */
export function nextPollMs<T>(state: PollState<T>, now: number, finished: (data: T) => boolean): number | false {
  const data = state.data;
  if (data === undefined) return state.status === "error" ? false : POLL_MS;
  if (finished(data)) return false;
  if (state.status !== "error") return POLL_MS;
  return Math.min(Math.max(POLL_MS, now - state.dataUpdatedAt), POLL_MAX_MS);
}

/** An execution read is final once the execution is terminal. */
export function executionFinished(data: workflowGetExecutionResponse): boolean {
  return data.status === 200 && isTerminalState(data.data.state);
}

/** `nextPollMs` for `GET …/executions/{id}`. */
export function nextExecutionPollMs(state: PollState<workflowGetExecutionResponse>, now: number): number | false {
  return nextPollMs(state, now, executionFinished);
}

/**
 * What a failed poll says after the error: the page keeps the last answer
 * (FE-STATE-1) and keeps trying. Also this module's marker in
 * `scripts/assert-workflows-lazy.mjs`.
 */
export function staleNote(dataUpdatedAt: number): string {
  return `What is shown is the last answer, read ${fullTimestamp(new Date(dataUpdatedAt).toISOString())}; it keeps trying on its own.`;
}

/** A clock for "… ago" labels and running durations; ticks once a second. */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}
