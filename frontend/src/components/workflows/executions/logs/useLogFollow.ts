import { useCallback, useEffect, useRef, useState } from "react";
import { workflowGetExecutionLogs } from "@/api/generated/workflow/workflow";
import type { LogLevel, LogLineDto, LogPageDto, WorkflowGetExecutionLogsParams } from "@/api/generated/model";
import { nextPollMs } from "@/components/workflows/executions/polling";

/**
 * FOLLOWING AN EXECUTION'S LOG BY CURSOR (W7b spec §7, the W11 contract).
 *
 * `GET …/executions/{id}/logs?after=` answers the settled lines after a
 * cursor, in id order, with `next_after` (the last id, or `null` when nothing
 * came back: keep the cursor you sent) and `complete`. The reader lags the
 * writer by 2 s, so a terminal `execution_state` can still have lines to come:
 * **polling stops only on `complete && items.length === 0`.**
 *
 * - A full page means more lines are already settled: the next page is read at
 *   once, so opening a long log catches up without waiting 2 s per page.
 * - Otherwise it reads again every 2 s (`POLL_MS`), backing off to 30 s while
 *   reads fail (`nextPollMs`), and not at all while the document is hidden.
 * - A failed read keeps every line already loaded and exposes the error and a
 *   retry (FE-STATE-1).
 * - A filter change starts again from the beginning: the cursor is per filter.
 *
 * **The window.** At most `windowLines` lines are held. Past that, the oldest
 * pages are dropped, and each dropped page's starting cursor is kept, so
 * "Load earlier" can read it again (`loadEarlier`): the cursor only moves
 * forward, but re-reading from a page's own `after` returns that page. After
 * the first "Load earlier", nothing more is set aside: the user asked for the
 * history, and the next page must not push it out again.
 */

/** Lines per read (spec §7: "Pages are 500 lines"). */
export const LOG_PAGE_LIMIT = 500;
/** The most lines held at once before the oldest pages are dropped. */
export const LOG_WINDOW_LINES = 20_000;

export interface LogFilters {
  /** Only lines at this level or above. */
  minLevel?: LogLevel;
  /** Only lines of this task (the YAML's task `id`). */
  taskId?: string;
  /** Only lines of this task run. */
  taskRunId?: string;
}

export interface LogFollowOptions {
  pageSize?: number;
  windowLines?: number;
}

export interface LogFollow {
  /** The loaded lines, in id order. */
  lines: LogLineDto[];
  /** No page has been read yet for these filters (and none failed). */
  loading: boolean;
  /** The last read failed; the lines are kept. `null` once a read succeeds. */
  error: unknown;
  /** Read again now. */
  retry: () => void;
  /** Every line has been served: the last page was `complete` and empty. */
  done: boolean;
  /** The execution's state as the last page read it, for display only. */
  executionState: string | undefined;
  /** Earlier lines were dropped from the window and can be read again. */
  hasEarlier: boolean;
  loadEarlier: () => void;
  loadingEarlier: boolean;
  earlierError: unknown;
}

interface Page {
  /** The cursor this page was read with; `undefined` for the first. */
  after: number | undefined;
  lines: LogLineDto[];
}

/** The query for one read: a filter left unset is not sent (orval would send `null` as the string "null"). */
export function logParams(filters: LogFilters, after: number | undefined, limit: number): WorkflowGetExecutionLogsParams {
  const p: WorkflowGetExecutionLogsParams = { limit };
  if (after !== undefined) p.after = after;
  if (filters.minLevel) p.min_level = filters.minLevel;
  if (filters.taskId) p.task_id = filters.taskId;
  if (filters.taskRunId) p.task_run_id = filters.taskRunId;
  return p;
}

async function readPage(
  ws: string,
  id: string,
  params: WorkflowGetExecutionLogsParams,
  signal?: AbortSignal,
): Promise<LogPageDto> {
  // `apiFetch` throws an `ApiError` on any other status; this narrows the type.
  const res = await workflowGetExecutionLogs(ws, id, params, { signal });
  if (res.status !== 200) throw new Error("The logs could not be read.");
  return res.data;
}

/** A page is final when the execution is complete and nothing more came back. */
const pageFinished = (page: LogPageDto) => page.complete && page.items.length === 0;

interface Snapshot {
  /** The read this snapshot belongs to; a snapshot of other filters is not shown. */
  key: string;
  lines: LogLineDto[];
  loading: boolean;
  error: unknown;
  done: boolean;
  executionState: string | undefined;
  hasEarlier: boolean;
  loadingEarlier: boolean;
  earlierError: unknown;
}

const INITIAL: Snapshot = {
  key: "",
  lines: [],
  loading: true,
  error: null,
  done: false,
  executionState: undefined,
  hasEarlier: false,
  loadingEarlier: false,
  earlierError: null,
};

export function useLogFollow(ws: string, executionId: string, filters: LogFilters, options: LogFollowOptions = {}): LogFollow {
  const pageSize = options.pageSize ?? LOG_PAGE_LIMIT;
  const windowLines = options.windowLines ?? LOG_WINDOW_LINES;
  const { minLevel, taskId, taskRunId } = filters;
  const key = JSON.stringify([ws, executionId, minLevel ?? null, taskId ?? null, taskRunId ?? null, pageSize, windowLines]);
  const [snap, setSnap] = useState<Snapshot>(INITIAL);
  const controls = useRef<{ retry: () => void; loadEarlier: () => void }>({ retry: () => {}, loadEarlier: () => {} });

  useEffect(() => {
    const f: LogFilters = { minLevel, taskId, taskRunId };
    const abort = new AbortController();
    let cursor: number | undefined;
    const pages: Page[] = [];
    /** The starting cursors of dropped pages, the most recently dropped last. */
    const earlier: (number | undefined)[] = [];
    /**
     * Once the user has brought earlier lines back, nothing is set aside any
     * more: the next page would otherwise push out what they just asked for.
     */
    let capacity = windowLines;
    let lastPage: LogPageDto | undefined;
    let lastGoodAt = 0;
    let error: unknown = null;
    let done = false;
    let inFlight = false;
    let waitingForVisible = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let loadingEarlier = false;
    let earlierError: unknown = null;

    const publish = () => {
      if (abort.signal.aborted) return;
      setSnap({
        key,
        lines: pages.length === 1 ? pages[0].lines : pages.flatMap((p) => p.lines),
        loading: lastPage === undefined && error === null,
        error,
        done,
        executionState: lastPage?.execution_state,
        hasEarlier: earlier.length > 0,
        loadingEarlier,
        earlierError,
      });
    };

    const trim = () => {
      let total = pages.reduce((n, p) => n + p.lines.length, 0);
      while (total > capacity && pages.length > 1) {
        const dropped = pages.shift()!;
        earlier.push(dropped.after);
        total -= dropped.lines.length;
      }
    };

    const schedule = () => {
      if (done || abort.signal.aborted) return;
      // A full page: more lines have settled already, read them now.
      const wait =
        error === null && lastPage !== undefined && lastPage.items.length >= pageSize
          ? 0
          : nextPollMs(
              { data: lastPage, status: error === null ? "success" : "error", dataUpdatedAt: lastGoodAt },
              Date.now(),
              pageFinished,
            );
      if (wait === false) return;
      timer = setTimeout(poll, wait);
    };

    async function poll() {
      timer = undefined;
      if (inFlight || done || abort.signal.aborted) return;
      // A hidden tab reads nothing; `visibilitychange` resumes it.
      if (typeof document !== "undefined" && document.visibilityState === "hidden") {
        waitingForVisible = true;
        return;
      }
      inFlight = true;
      try {
        const page = await readPage(ws, executionId, logParams(f, cursor, pageSize), abort.signal);
        if (abort.signal.aborted) return;
        if (page.items.length > 0) {
          pages.push({ after: cursor, lines: page.items });
          trim();
        }
        // `null` when nothing came back: keep the cursor that was sent.
        cursor = page.next_after ?? cursor;
        lastPage = page;
        lastGoodAt = Date.now();
        error = null;
        done = pageFinished(page);
      } catch (e) {
        if (abort.signal.aborted) return;
        error = e;
      } finally {
        inFlight = false;
      }
      publish();
      schedule();
    }

    const onVisibility = () => {
      if (document.visibilityState !== "hidden" && waitingForVisible) {
        waitingForVisible = false;
        void poll();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);

    controls.current = {
      retry: () => {
        if (inFlight || done) return;
        clearTimeout(timer);
        waitingForVisible = false;
        void poll();
      },
      loadEarlier: () => {
        if (loadingEarlier || earlier.length === 0 || pages.length === 0) return;
        const after = earlier[earlier.length - 1];
        const firstKept = pages[0].lines[0]?.id ?? Number.POSITIVE_INFINITY;
        loadingEarlier = true;
        earlierError = null;
        publish();
        void (async () => {
          try {
            const page = await readPage(ws, executionId, logParams(f, after, pageSize), abort.signal);
            if (abort.signal.aborted) return;
            earlier.pop();
            // The re-read may run past the dropped page into lines already held.
            const lines = page.items.filter((l) => l.id < firstKept);
            if (lines.length > 0) pages.unshift({ after, lines });
            capacity = Number.POSITIVE_INFINITY;
          } catch (e) {
            if (abort.signal.aborted) return;
            earlierError = e;
          } finally {
            loadingEarlier = false;
          }
          publish();
        })();
      },
    };

    void poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [key, ws, executionId, minLevel, taskId, taskRunId, pageSize, windowLines]);

  const retry = useCallback(() => controls.current.retry(), []);
  const loadEarlier = useCallback(() => controls.current.loadEarlier(), []);
  // Until the first answer for new filters, the old filters' lines are not shown as theirs.
  const current = snap.key === key ? snap : INITIAL;
  return { ...current, retry, loadEarlier };
}

/** The most lines a download holds; the UI says so when it stops there. */
export const LOG_DOWNLOAD_MAX_LINES = 100_000;
/** Lines per read while downloading: the route's largest page. */
export const LOG_DOWNLOAD_PAGE = 2_000;

/** One line of the `.log` file: time, level, where it came from, then the message as written. */
export function formatLogLine(l: LogLineDto): string {
  const where = l.task_id ? ` [${l.task_id}${l.attempt != null ? ` attempt ${l.attempt + 1}` : ""}]` : "";
  return `${l.ts} ${l.level.padEnd(5)}${where} ${l.message}`;
}

export interface LogDownload {
  text: string;
  lines: number;
  /** It stopped at `maxLines`; the execution has more. */
  capped: boolean;
}

/**
 * Every line the filters select, read page by page from the start, up to
 * `maxLines`. It stops at the first empty page: the lines settled so far.
 */
export async function downloadLogs(
  ws: string,
  executionId: string,
  filters: LogFilters,
  { pageSize = LOG_DOWNLOAD_PAGE, maxLines = LOG_DOWNLOAD_MAX_LINES }: { pageSize?: number; maxLines?: number } = {},
): Promise<LogDownload> {
  const out: string[] = [];
  let after: number | undefined;
  for (;;) {
    const page = await readPage(ws, executionId, logParams(filters, after, Math.min(pageSize, maxLines - out.length + 1)));
    for (const l of page.items) {
      if (out.length === maxLines) return { text: out.join("\n") + "\n", lines: out.length, capped: true };
      out.push(formatLogLine(l));
    }
    if (page.items.length === 0 || page.next_after == null) break;
    after = page.next_after;
  }
  return { text: out.length ? out.join("\n") + "\n" : "", lines: out.length, capped: false };
}
