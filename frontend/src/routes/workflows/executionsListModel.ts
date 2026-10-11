import type { WorkflowListExecutionsParams } from "@/api/generated/model";

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
 * | `subns` | `1`: the namespace's child namespaces too (`namespace_prefix`) | that namespace only |
 * | `flow` | a flow's uuid | every flow |
 * | `time` | `1h`, `24h`, `7d`, `30d` or `custom` | all time |
 * | `since`, `until` | UTC instants, with `time=custom` only | open |
 * | `child` | `1`: child executions too | no loop iterations or subflow and flow-trigger children (retries stay) |
 * | `page` | 1-based page | 1 |
 */

/** Rows per page (spec §4.1). */
export const PAGE_SIZE = 50;

/** How often a page with a live row is read again (spec §4.1). */
export const LIST_POLL_MS = 5000;

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
  /** Include the namespace's child namespaces (`namespace_prefix`). Meaningless without `namespace`. */
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

/**
 * The filters `params` holds. On a flow's own Executions tab (`pinned`) the
 * flow is fixed and the flow and namespace pickers are not shown, so a
 * `flow`, `namespace` or `subns` left in the URL (a link copied from the list
 * page, say) is read as absent: it neither narrows the request nor counts as
 * a filter the page would have to offer to clear.
 */
export function readFilters(params: URLSearchParams, opts: { pinned?: boolean } = {}): ExecutionFilters {
  if (opts.pinned) return { ...readFilters(params), namespace: "", subNamespaces: false, flow: "" };
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
 * The request for one page at `now`.
 *
 * - A time preset is computed against `now` and sends `since` only: an
 *   `until` of now would cut off every execution that starts while the page
 *   polls.
 * - A namespace is sent exactly, or with its child namespaces as
 *   `namespace_prefix` (dot-anchored on the server; the user's ruling,
 *   2026-10-11).
 * - `pinnedFlowId` is the flow's own Executions tab: it replaces the flow
 *   filter, and no namespace is sent.
 */
export function toQueryParams(
  f: ExecutionFilters,
  now: number,
  opts: { pinnedFlowId?: string } = {},
): WorkflowListExecutionsParams {
  const q: WorkflowListExecutionsParams = {};
  if (f.states.length) q.state = f.states.join(",");
  if (!opts.pinnedFlowId && f.namespace) {
    if (f.subNamespaces) q.namespace_prefix = f.namespace;
    else q.namespace = f.namespace;
  }
  const flowId = opts.pinnedFlowId ?? f.flow;
  if (flowId) q.flow_id = flowId;
  const preset = TIME_PRESETS.find((p) => p.value === f.time);
  if (preset) q.since = new Date(now - preset.ms).toISOString();
  if (f.time === "custom") {
    if (f.since) q.since = f.since;
    if (f.until) q.until = f.until;
  }
  if (!f.showChildren) q.root_only = true;
  q.limit = PAGE_SIZE;
  q.offset = (f.page - 1) * PAGE_SIZE;
  return q;
}

/** How many pages the pager offers: at least one. */
export function pageCount(total: number): number {
  return Math.max(1, Math.ceil(total / PAGE_SIZE));
}
