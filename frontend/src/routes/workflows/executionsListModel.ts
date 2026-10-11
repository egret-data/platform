import type { ExecutionListItemDto, ExecutionPageDto, WorkflowListExecutionsParams } from "@/api/generated/model";
import { namespaceContains } from "@/components/workflows/namespaceTree";

/**
 * THE EXECUTIONS LIST'S STATE: the URL, read into filters, turned into
 * requests (W7b spec §4.1). Pure, so every value is tested without a page.
 *
 * Every filter lives in the URL (FE-PAGE-2), under these keys:
 *
 * | key | value | default (absent) |
 * |---|---|---|
 * | `state` | states, one comma list | every state |
 * | `namespace` | one namespace | every namespace |
 * | `subns` | `1`: the namespace's known child namespaces too | that namespace only |
 * | `flow` | a flow's uuid | every flow |
 * | `time` | `1h`, `24h`, `7d`, `30d` or `custom` | all time |
 * | `since`, `until` | UTC instants, with `time=custom` only | open |
 * | `child` | `1`: child executions too | root executions only |
 * | `page` | 1-based page | 1 |
 */

/** Rows per page (spec §4.1). */
export const PAGE_SIZE = 50;

/** How often a page with a live row is read again (spec §4.1). */
export const LIST_POLL_MS = 5000;

/**
 * The most rows one request returns: the server clamps `limit` to 200
 * (`WorkflowListExecutionsParams.limit`). A fanned-out namespace query can
 * only merge pages exactly within this many rows of each namespace.
 */
export const MERGED_ROW_LIMIT = 200;

/** The seventeen Kestra execution states, in the state picker's order: pending, running, outcomes. */
export const EXECUTION_STATE_NAMES = [
  "CREATED",
  "SUBMITTED",
  "QUEUED",
  "RESTARTED",
  "RESUBMITTED",
  "RUNNING",
  "RETRYING",
  "KILLING",
  "PAUSED",
  "BREAKPOINT",
  "SUCCESS",
  "WARNING",
  "FAILED",
  "KILLED",
  "CANCELLED",
  "SKIPPED",
  "RETRIED",
] as const;

export type TimePreset = "1h" | "24h" | "7d" | "30d";

const HOUR = 3_600_000;
export const TIME_PRESETS: { value: TimePreset; label: string; ms: number }[] = [
  { value: "1h", label: "Last hour", ms: HOUR },
  { value: "24h", label: "Last 24 hours", ms: 24 * HOUR },
  { value: "7d", label: "Last 7 days", ms: 7 * 24 * HOUR },
  { value: "30d", label: "Last 30 days", ms: 30 * 24 * HOUR },
];

export interface ExecutionFilters {
  states: string[];
  namespace: string;
  /** Include the namespace's known child namespaces. Meaningless without `namespace`. */
  subNamespaces: boolean;
  /** A flow's uuid (`flow_id`), not its YAML id: those repeat across namespaces. */
  flow: string;
  time: TimePreset | "custom" | "";
  /** With `time: "custom"` only: UTC ISO instants, either may be empty. */
  since: string;
  until: string;
  /** "Show child executions": off sends `root_only=true`. */
  showChildren: boolean;
  /** 1-based. */
  page: number;
}

export const DEFAULT_FILTERS: ExecutionFilters = {
  states: [],
  namespace: "",
  subNamespaces: false,
  flow: "",
  time: "",
  since: "",
  until: "",
  showChildren: false,
  page: 1,
};

const OWNED_KEYS = ["state", "namespace", "subns", "flow", "time", "since", "until", "child", "page"];
const isTime = (v: string): v is ExecutionFilters["time"] => v === "custom" || TIME_PRESETS.some((p) => p.value === v);

export function readFilters(params: URLSearchParams): ExecutionFilters {
  const states = [...new Set((params.get("state") ?? "").split(",").filter(Boolean))];
  const namespace = params.get("namespace") ?? "";
  const time = params.get("time") ?? "";
  const custom = time === "custom";
  const page = Number(params.get("page"));
  return {
    states,
    namespace,
    subNamespaces: namespace !== "" && params.get("subns") === "1",
    flow: params.get("flow") ?? "",
    time: isTime(time) ? time : "",
    since: custom ? (params.get("since") ?? "") : "",
    until: custom ? (params.get("until") ?? "") : "",
    showChildren: params.get("child") === "1",
    page: Number.isInteger(page) && page >= 1 ? page : 1,
  };
}

/**
 * `params` with `patch` applied to its filters, written canonically: a
 * default is absent rather than spelled out. A patch that does not name a page
 * goes back to page 1, because the old page number means nothing under new
 * filters. Keys the list does not own are kept.
 */
export function withFilters(params: URLSearchParams, patch: Partial<ExecutionFilters>): URLSearchParams {
  const f: ExecutionFilters = { ...readFilters(params), page: 1, ...patch };
  const out = new URLSearchParams(params);
  for (const k of OWNED_KEYS) out.delete(k);
  if (f.states.length) out.set("state", [...new Set(f.states)].join(","));
  if (f.namespace) {
    out.set("namespace", f.namespace);
    if (f.subNamespaces) out.set("subns", "1");
  }
  if (f.flow) out.set("flow", f.flow);
  if (f.time) out.set("time", f.time);
  if (f.time === "custom") {
    if (f.since) out.set("since", f.since);
    if (f.until) out.set("until", f.until);
  }
  if (f.showChildren) out.set("child", "1");
  if (f.page > 1) out.set("page", String(f.page));
  return out;
}

/**
 * Whether a filter narrows the list, which is what tells "no executions yet"
 * from "nothing matches". The child switch is not one: every child has a root,
 * so no roots means no executions at all.
 */
export function isFiltering(f: ExecutionFilters): boolean {
  return f.states.length > 0 || f.namespace !== "" || f.flow !== "" || f.time !== "";
}

/** What "Clear filters" leaves: the view (the child switch) stays, the filters and page go. */
export function clearedFilters(f: ExecutionFilters): Partial<ExecutionFilters> {
  return { ...DEFAULT_FILTERS, showChildren: f.showChildren };
}

/**
 * The namespaces to ask for: none (no filter), the chosen one, or, with its
 * child namespaces, every known flow namespace beneath it plus itself.
 *
 * The API matches a namespace exactly (`namespace = $3`), so "include
 * children" is a client-side expansion over the namespaces the client knows
 * from the flow list (spec §4.1, §15): a namespace whose every flow was deleted
 * is not known, and is not asked for.
 */
export function namespacesToQuery(f: ExecutionFilters, flowNamespaces: readonly string[]): string[] {
  if (!f.namespace) return [];
  if (!f.subNamespaces) return [f.namespace];
  const set = new Set([f.namespace, ...flowNamespaces.filter((ns) => namespaceContains(f.namespace, ns))]);
  return [...set].sort((a, b) => a.localeCompare(b));
}

export interface RequestPlan {
  requests: WorkflowListExecutionsParams[];
  /** The page's offset into the whole match. */
  offset: number;
  /** Several namespaces were asked from offset 0; `mergePages` interleaves them. */
  merged: boolean;
}

/**
 * The requests for one page at `now`.
 *
 * - A time preset is computed against `now` and sends `since` only: an
 *   `until` of now would cut off every execution that starts while the page
 *   polls.
 * - `pinnedFlowId` is the flow's own Executions tab: it replaces the flow and
 *   namespace filters.
 * - Several namespaces fan out, one request each, from offset 0 and as deep as
 *   the page reaches (capped at `MERGED_ROW_LIMIT`), so the merged rows before
 *   the page's end are exact.
 */
export function planRequests(
  f: ExecutionFilters,
  now: number,
  namespaces: readonly string[],
  opts: { pinnedFlowId?: string } = {},
): RequestPlan {
  const common: WorkflowListExecutionsParams = {};
  if (f.states.length) common.state = f.states.join(",");
  const flowId = opts.pinnedFlowId ?? f.flow;
  if (flowId) common.flow_id = flowId;
  const preset = TIME_PRESETS.find((p) => p.value === f.time);
  if (preset) common.since = new Date(now - preset.ms).toISOString();
  if (f.time === "custom") {
    if (f.since) common.since = f.since;
    if (f.until) common.until = f.until;
  }
  if (!f.showChildren) common.root_only = true;

  const offset = (f.page - 1) * PAGE_SIZE;
  const scoped = opts.pinnedFlowId ? [] : namespaces;
  if (scoped.length <= 1) {
    const ns = scoped[0];
    return { requests: [{ ...(ns ? { namespace: ns } : {}), ...common, limit: PAGE_SIZE, offset }], offset, merged: false };
  }
  const limit = Math.min(MERGED_ROW_LIMIT, offset + PAGE_SIZE);
  return { requests: scoped.map((namespace) => ({ namespace, ...common, limit, offset: 0 })), offset, merged: true };
}

/** `created_at` as a sortable key: microseconds kept, which a JS `Date` would drop. */
function createdKey(iso: string): string {
  const m = /^(.*T\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:?\d\d)?$/.exec(iso);
  if (!m) return iso;
  const ms = Date.parse(`${m[1]}${m[3] ?? "Z"}`);
  return `${String(ms).padStart(15, "0")}.${(m[2] ?? "").padEnd(9, "0")}`;
}

/**
 * One page out of the answers: a single answer as is, or fanned-out answers
 * interleaved in the server's own order (`created_at DESC, id DESC`, `repo.rs`)
 * with their totals summed.
 */
export function mergePages(
  pages: ExecutionPageDto[],
  offset: number,
  merged: boolean,
): { rows: ExecutionListItemDto[]; total: number } {
  if (!merged) return { rows: pages[0]?.rows ?? [], total: pages[0]?.total ?? 0 };
  const all = pages.flatMap((p) => p.rows);
  all.sort((a, b) => {
    const ka = createdKey(a.created_at);
    const kb = createdKey(b.created_at);
    if (ka !== kb) return ka < kb ? 1 : -1;
    return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
  });
  return { rows: all.slice(offset, offset + PAGE_SIZE), total: pages.reduce((n, p) => n + p.total, 0) };
}

/** How many pages the pager offers: at least one, and no more than a fan-out can merge exactly. */
export function pageCount(total: number, merged: boolean): number {
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  return merged ? Math.min(pages, MERGED_ROW_LIMIT / PAGE_SIZE) : pages;
}
