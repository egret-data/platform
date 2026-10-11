import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type UIEvent } from "react";
import dayjs from "dayjs";
import { ArrowDown, ChevronDown, ChevronRight, ChevronsDownUp, ChevronsUpDown, Download, History, Loader2, RotateCw, ScrollText } from "lucide-react";
import type { LogLevel, LogLineDto, TaskRunDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  buildRows,
  EXECUTION_GROUP,
  rowAt,
  rowHeight,
  type LogGroup,
  type LogRow,
} from "@/components/workflows/executions/logs/logRows";
import {
  downloadLogs,
  LOG_DOWNLOAD_MAX_LINES,
  useLogFollow,
  type LogFilters,
} from "@/components/workflows/executions/logs/useLogFollow";

/**
 * THE LOG VIEW (W7b spec §7), shared by the Logs tab and the Gantt's run
 * panel. Kestra's `executions/Logs.vue` and `logs/TaskRunDetails.vue`: lines
 * grouped by task run, each group collapsible, the execution's own lines
 * first; a toolbar with expand/collapse all and Download. egret's visuals.
 *
 * **Windowed, not a library.** The repo has no virtualisation library, and
 * this list needs very little of one: every row's height is known from its
 * message (`logRows.ts`), so the list renders the rows the viewport covers
 * (plus `OVERSCAN_PX` either side) between two spacers.
 *
 * **Following.** Lines are read by `useLogFollow`. While the user is at the
 * bottom, new lines scroll into view; once they scroll up, the view stays put
 * and "Jump to latest" appears.
 */

/** Rendered beyond the viewport on each side, so a fast scroll does not show a gap. */
export const OVERSCAN_PX = 600;
/** Within this of the bottom counts as at the bottom. */
export const STICK_PX = 8;
/** Assumed until the scroller is measured (and in jsdom, which has no layout). */
const DEFAULT_VIEWPORT_PX = 480;

const LEVEL_TONE: Record<LogLevel, string> = {
  // The tint carries the tone, the ink is `--foreground` (StateChip's rule):
  // coloured 12px ink on its own wash fails 4.5:1. The word is the information.
  ERROR: "bg-destructive/15",
  WARN: "bg-highlight/15",
  INFO: "bg-primary/15",
  DEBUG: "bg-muted",
  TRACE: "bg-muted-foreground/15",
};

/** A level's badge: the level's name, so it is never colour alone (FE-A11Y-4). */
export function LevelBadge({ level }: { level: LogLevel }) {
  return (
    <span
      data-testid="log-level"
      className={cn("inline-block w-12 shrink-0 rounded px-1 text-center font-mono text-xs leading-4 text-foreground", LEVEL_TONE[level])}
    >
      {level}
    </span>
  );
}

function groupLabel(g: LogGroup): string {
  if (g.key === EXECUTION_GROUP) return "Execution";
  const run = g.runIx === null ? "" : ` #${g.runIx}`;
  return `${g.taskId ?? "Unknown task"}${run}${g.value ? ` (${g.value})` : ""}`;
}

/** Save `text` as a file, through a temporary link. */
function saveText(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

export interface LogViewProps {
  ws: string;
  executionId: string;
  filters: LogFilters;
  /** The execution's runs: group labels, their order, and attempt counts. */
  runs?: TaskRunDto[];
  /** Group by task run (the Logs tab). A single run's panel shows its lines alone. */
  grouped?: boolean;
  /** The user narrowed the lines; an empty result then says so and offers `onClearFilters`. */
  filtersActive?: boolean;
  onClearFilters?: () => void;
  /** The scroller's height classes. */
  heightClass?: string;
  /** The list's accessible name. */
  label?: string;
  /** Lines per read and the window, for tests. */
  pageSize?: number;
  windowLines?: number;
  /** A download's page and cap, for tests. */
  downloadPageSize?: number;
  downloadMaxLines?: number;
}

export function LogView({
  ws,
  executionId,
  filters,
  runs,
  grouped = true,
  filtersActive = false,
  onClearFilters,
  heightClass = "h-[28rem]",
  label = "Log lines",
  pageSize,
  windowLines,
  downloadPageSize,
  downloadMaxLines = LOG_DOWNLOAD_MAX_LINES,
}: LogViewProps) {
  const follow = useLogFollow(ws, executionId, filters, { pageSize, windowLines });
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const { rows, offsets } = useMemo(
    () => buildRows(follow.lines, { grouped, runs, collapsed }),
    [follow.lines, grouped, runs, collapsed],
  );
  const groupKeys = useMemo(() => rows.flatMap((r) => (r.kind === "group" ? [r.group.key] : [])), [rows]);
  const allCollapsed = groupKeys.length > 0 && groupKeys.every((k) => collapsed.has(k));

  const toggle = useCallback((key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const [download, setDownload] = useState<{ busy: boolean; capped: boolean; error: string | null }>({
    busy: false,
    capped: false,
    error: null,
  });
  const runDownload = async () => {
    setDownload({ busy: true, capped: false, error: null });
    try {
      const file = await downloadLogs(ws, executionId, filters, { pageSize: downloadPageSize, maxLines: downloadMaxLines });
      saveText(`${executionId}.log`, file.text);
      setDownload({ busy: false, capped: file.capped, error: null });
    } catch (e) {
      setDownload({ busy: false, capped: false, error: errorMessage(e) });
    }
  };

  // Error first (FE-STATE-1): only while nothing is loaded; a failed poll keeps the lines.
  let body;
  if (follow.error && follow.lines.length === 0) {
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>The logs could not be loaded: {errorMessage(follow.error)}. Try again.</span>
          <Button variant="outline" size="sm" onClick={follow.retry}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  } else if (follow.loading) {
    body = (
      <div data-testid="logs-loading">
        <ListSkeleton rows={6} cols={3} />
      </div>
    );
  } else if (follow.lines.length === 0) {
    const clear = filtersActive && onClearFilters ? (
      <Button variant="outline" size="sm" onClick={onClearFilters}>
        Clear filters
      </Button>
    ) : undefined;
    body = follow.done ? (
      filtersActive ? (
        <EmptyState icon={ScrollText} title="No lines match the filters" description="Clear the filters to see every line." action={clear} />
      ) : (
        <EmptyState icon={ScrollText} title="No logs" description="The execution has finished without writing any log lines." />
      )
    ) : (
      <EmptyState
        icon={ScrollText}
        title="No logs yet"
        description={
          filtersActive
            ? "No line matches the filters yet. Lines appear about 2 seconds after they are written."
            : "Lines appear here about 2 seconds after the tasks write them."
        }
        action={clear}
      />
    );
  } else {
    body = (
      <>
        {follow.error ? (
          <Alert variant="destructive" className="mb-2">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>
                The logs could not be refreshed: {errorMessage(follow.error)}. The lines shown are kept, and it keeps trying on
                its own.
              </span>
              <Button variant="outline" size="sm" onClick={follow.retry}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}
        {follow.hasEarlier || follow.earlierError ? (
          <div className="mb-2 flex flex-wrap items-center gap-3 text-sm">
            {follow.hasEarlier ? (
              <Button variant="outline" size="sm" disabled={follow.loadingEarlier} onClick={follow.loadEarlier}>
                {follow.loadingEarlier ? <Loader2 aria-hidden className="animate-spin" /> : <History aria-hidden />}
                Load earlier
              </Button>
            ) : null}
            <span className="text-muted-foreground">
              {follow.earlierError
                ? `Earlier lines could not be loaded: ${errorMessage(follow.earlierError)}. Try again.`
                : "Older lines were set aside to keep the view fast."}
            </span>
          </div>
        ) : null}
        <WindowedLines rows={rows} offsets={offsets} heightClass={heightClass} label={label} onToggle={toggle} />
      </>
    );
  }

  return (
    <div data-testid="log-view" className="flex min-w-0 flex-col">
      <div className="mb-2 flex flex-wrap items-center justify-end gap-2">
        {grouped && groupKeys.length > 0 ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setCollapsed(allCollapsed ? new Set() : new Set(groupKeys))}
          >
            {allCollapsed ? <ChevronsUpDown aria-hidden /> : <ChevronsDownUp aria-hidden />}
            {allCollapsed ? "Expand all" : "Collapse all"}
          </Button>
        ) : null}
        <Button
          variant="outline"
          size="sm"
          disabled={download.busy}
          title={`Saves every line the filters select, up to ${downloadMaxLines.toLocaleString("en-US")} lines.`}
          onClick={() => void runDownload()}
        >
          {download.busy ? <Loader2 aria-hidden className="animate-spin" /> : <Download aria-hidden />}
          {download.busy ? "Downloading…" : "Download"}
        </Button>
      </div>
      {download.error ? (
        <p role="alert" className="mb-2 text-sm text-destructive">
          The logs could not be downloaded: {download.error}. Try again.
        </p>
      ) : null}
      {download.capped ? (
        <p role="status" className="mb-2 text-sm text-muted-foreground">
          The file holds the first {downloadMaxLines.toLocaleString("en-US")} lines; the execution has more. Filter by task,
          run or level to download the rest.
        </p>
      ) : null}
      {body}
    </div>
  );
}

/** Which rows' heights an anchor is measured against: the row at the top of the viewport, and how far into it. */
interface Anchor {
  key: string;
  delta: number;
}

function WindowedLines({
  rows,
  offsets,
  heightClass,
  label,
  onToggle,
}: {
  rows: LogRow[];
  offsets: number[];
  heightClass: string;
  label: string;
  onToggle: (key: string) => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(DEFAULT_VIEWPORT_PX);
  const [atBottom, setAtBottom] = useState(true);
  /** Follow new lines: true while the user is at the bottom. */
  const stick = useRef(true);
  const anchor = useRef<Anchor | null>(null);
  const total = offsets[rows.length];

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const measure = () => setViewport(el.clientHeight || DEFAULT_VIEWPORT_PX);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const keyIndex = useMemo(() => new Map(rows.map((r, i) => [r.key, i])), [rows]);

  // After the rows change: follow the bottom, or keep the row the user was
  // reading where it was (lines set aside above it, earlier lines loaded,
  // a group collapsed).
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    if (stick.current) {
      el.scrollTop = total;
      setScrollTop(el.scrollTop);
      return;
    }
    const a = anchor.current;
    const i = a ? keyIndex.get(a.key) : undefined;
    if (a && i !== undefined) {
      const top = offsets[i] + a.delta;
      if (top !== el.scrollTop) {
        el.scrollTop = top;
        setScrollTop(top);
      }
    }
  }, [rows, offsets, keyIndex, total]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const top = el.scrollTop;
    const vh = el.clientHeight || viewport;
    const bottom = top + vh >= total - STICK_PX;
    stick.current = bottom;
    setAtBottom(bottom);
    setScrollTop(top);
    const i = rowAt(offsets, top);
    anchor.current = rows[i] ? { key: rows[i].key, delta: top - offsets[i] } : null;
  };

  const jumpToLatest = () => {
    const el = scroller.current;
    stick.current = true;
    setAtBottom(true);
    if (el) {
      el.scrollTop = total;
      setScrollTop(el.scrollTop);
    }
  };

  const first = rows.length === 0 ? 0 : rowAt(offsets, Math.max(0, scrollTop - OVERSCAN_PX));
  const last = rows.length === 0 ? -1 : rowAt(offsets, scrollTop + viewport + OVERSCAN_PX);
  const visible = rows.slice(first, last + 1);

  return (
    <div className="relative">
      {/* No `tabIndex`: jsx-a11y refuses one on a non-interactive role, and
          browsers make a scroller with no focusable child a tab stop
          themselves (Chromium 130+, Firefox), so the run panel's list is
          reachable; grouped, Tab reaches the group headers inside it, and the
          arrow keys then scroll it (FE-A11Y-3; measured in the task report).
          `aria-live` off: a live region would read every polled page aloud. */}
      <div
        ref={scroller}
        role="log"
        aria-live="off"
        aria-label={label}
        data-testid="log-scroller"
        onScroll={onScroll}
        className={cn(
          "overflow-auto rounded-md border bg-card font-mono text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          heightClass,
        )}
      >
        <div style={{ height: offsets[first] ?? 0 }} aria-hidden />
        {visible.map((row) =>
          row.kind === "group" ? (
            <GroupRow key={row.key} row={row} onToggle={onToggle} />
          ) : (
            <LineRow key={row.key} line={row.line} showAttempt={row.showAttempt} />
          ),
        )}
        <div style={{ height: total - (offsets[last + 1] ?? total) }} aria-hidden />
      </div>
      {atBottom ? null : (
        <Button size="sm" className="absolute bottom-3 right-5 shadow-md" onClick={jumpToLatest}>
          <ArrowDown aria-hidden />
          Jump to latest
        </Button>
      )}
    </div>
  );
}

function GroupRow({ row, onToggle }: { row: Extract<LogRow, { kind: "group" }>; onToggle: (key: string) => void }) {
  const g = row.group;
  const Icon = row.collapsed ? ChevronRight : ChevronDown;
  return (
    <div data-testid="log-group" className="sticky left-0 flex w-full items-center border-b bg-muted/60 px-2" style={{ height: rowHeight(row) }}>
      <button
        type="button"
        aria-expanded={!row.collapsed}
        onClick={() => onToggle(g.key)}
        className="flex items-center gap-1.5 rounded px-1 font-sans text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <Icon aria-hidden className="size-4" />
        <span>{groupLabel(g)}</span>
        <span className="font-normal tabular-nums text-muted-foreground">
          {g.lines.length === 1 ? "1 line" : `${g.lines.length.toLocaleString("en-US")} lines`}
        </span>
      </button>
    </div>
  );
}

function LineRow({ line, showAttempt }: { line: LogLineDto; showAttempt: boolean }) {
  return (
    <div
      data-testid="log-line"
      className="flex w-max min-w-full items-start gap-2 px-2 py-0.5 leading-5 hover:bg-accent/50"
      style={{ height: rowHeight({ kind: "line", key: "", line, showAttempt }) }}
    >
      <time dateTime={line.ts} title={line.ts} className="shrink-0 tabular-nums text-muted-foreground">
        {dayjs(line.ts).format("HH:mm:ss.SSS")}
      </time>
      <span className="flex h-5 shrink-0 items-center">
        <LevelBadge level={line.level} />
      </span>
      {showAttempt && line.attempt != null ? (
        <span data-testid="log-attempt" className="shrink-0 tabular-nums text-muted-foreground">
          Attempt {line.attempt + 1}
        </span>
      ) : null}
      <pre className="m-0 whitespace-pre font-mono text-xs leading-5">{line.message}</pre>
    </div>
  );
}
