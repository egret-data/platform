import {
  Box,
  Clock,
  Database,
  FolderOpen,
  GitBranch,
  Globe,
  ListOrdered,
  Network,
  Pause,
  RefreshCw,
  Repeat,
  ScrollText,
  ShieldAlert,
  Split,
  Signpost,
  SquareTerminal,
  Webhook,
  Workflow,
  Zap,
  type LucideIcon,
} from "lucide-react";
import type { Branch, Cluster, ClusterKind, ExpansionReason, Node, NodeState, Relation } from "@/api/generated/model";
import { formatDuration, isTerminalState } from "@/components/workflows/executionState";

/**
 * WHAT A TOPOLOGY NODE SAYS (W7c spec §6.1), shared by the canvas, the list
 * view and the layout (which sizes a card by how many lines it holds).
 */

export type TaskNode = Extract<Node, { node: "task" }>;
export type TriggerNode = Extract<Node, { node: "trigger" }>;

/** `io.kestra.plugin.core.log.Log` → `Log`; a short alias stays as written. */
export function shortType(type: string): string {
  return type.slice(type.lastIndexOf(".") + 1) || type;
}

/** A construct's name in words, for a cluster's header and a collapsed cluster. */
export const CLUSTER_WORD: Record<ClusterKind, string> = {
  sequential: "Sequential",
  parallel: "Parallel",
  if: "If",
  switch: "Switch",
  dag: "Dag",
  loop: "Loop",
  loop_until: "Loop until",
  allow_failure: "Allow failure",
  working_directory: "Working directory",
  pause: "Pause",
  triggers: "Triggers",
  subflow: "Subflow",
};

const CLUSTER_ICON: Record<ClusterKind, LucideIcon> = {
  sequential: ListOrdered,
  parallel: Split,
  if: GitBranch,
  switch: Signpost,
  dag: Network,
  loop: Repeat,
  loop_until: RefreshCw,
  allow_failure: ShieldAlert,
  working_directory: FolderOpen,
  pause: Pause,
  triggers: Zap,
  subflow: Workflow,
};

export interface TypeIcon {
  icon: LucideIcon;
  /** A stable name for the glyph (`data-icon`), so a test can tell which one is drawn. */
  name: string;
}

/**
 * A node's glyph. egret has no plugin icons (Kestra draws each plugin's own),
 * so a flowable takes its construct's, a `Subflow` the workflow glyph, a
 * trigger its kind's, and a task a glyph for the family its type names.
 */
export function typeIcon(node: TaskNode | TriggerNode, clusterKind?: ClusterKind): TypeIcon {
  const type = (node.node === "task" ? node.canonical_type : node.type).toLowerCase();
  if (node.node === "trigger") {
    if (type.includes("schedule")) return { icon: Clock, name: "schedule" };
    if (type.includes("webhook")) return { icon: Webhook, name: "webhook" };
    if (type.endsWith(".flow") || type === "flow") return { icon: Workflow, name: "flow-trigger" };
    return { icon: Zap, name: "trigger" };
  }
  if (clusterKind && clusterKind !== "subflow") return clusterIcon(clusterKind);
  if (node.kind === "executable") return { icon: Workflow, name: "subflow" };
  const last = shortType(type);
  if (last === "log" || type.includes(".log.")) return { icon: ScrollText, name: "log" };
  if (type.includes("http")) return { icon: Globe, name: "http" };
  if (/jdbc|sql|query|duckdb|postgres|trino|starrocks/.test(type)) return { icon: Database, name: "sql" };
  if (/script|python|shell|commands|bash|node\b/.test(type)) return { icon: SquareTerminal, name: "script" };
  if (last === "pause") return { icon: Pause, name: "pause" };
  return { icon: Box, name: "task" };
}

/** A cluster's glyph, for a collapsed cluster's card and the list view's cluster rows. */
export function clusterIcon(kind: ClusterKind): TypeIcon {
  return { icon: CLUSTER_ICON[kind], name: kind.replace("_", "-") };
}

/** The flowable's own cluster kind, when the node owns one. */
export function clusterKindOf(uid: string, clusters: readonly Cluster[]): ClusterKind | undefined {
  return clusters.find((c) => c.uid === `cluster:${uid}` && !c.reason)?.kind;
}

/** A branch other than `tasks`, in words and in its token (spec §6.1, "Marking"). */
export const BRANCH_LOOK: Partial<Record<Branch, { word: string; colour: string }>> = {
  errors: { word: "On error", colour: "var(--destructive)" },
  finally: { word: "Finally", colour: "var(--highlight)" },
  after_execution: { word: "After execution", colour: "var(--highlight)" },
};

/** An edge's stroke: `errors` in the danger token, `finally` and `afterExecution` in the warning token. */
export function relationStroke(relation: Relation): string {
  if (relation === "error") return "var(--destructive)";
  if (relation === "finally" || relation === "after_execution") return "var(--highlight)";
  return "var(--muted-foreground)";
}

/** Why a `Subflow` did not expand in place (spec §3.4). */
export const EXPANSION_REASON: Record<ExpansionReason, string> = {
  not_found: "No flow by that namespace and id in this workspace",
  dynamic: "Its namespace or flow id is an expression, so only a run names the child flow",
  depth: "Not expanded: Subflows expand two levels deep",
};

/** The note on a task egret refuses to run (no-silent-stall); its code is shown beside it. */
export const REFUSAL_NOTE = "egret does not run this task";

/**
 * A `Loop` body's iterations, as the spec words it: "2 of 3 iterations done,
 * 1 failed". "Done" is an iteration that finished without failing (a terminal
 * state other than `FAILED` and `KILLED`); failed and killed ones are counted
 * apart, and a running one is neither.
 */
export function iterationsText(it: NonNullable<NodeState["iterations"]>): string {
  let done = 0;
  for (const [s, n] of Object.entries(it.by_state)) if (isTerminalState(s) && s !== "FAILED" && s !== "KILLED") done += n;
  const failed = it.by_state.FAILED ?? 0;
  const killed = it.by_state.KILLED ?? 0;
  const noun = it.total === 1 ? "iteration" : "iterations";
  return `${done} of ${it.total} ${noun} done${failed > 0 ? `, ${failed} failed` : ""}${killed > 0 ? `, ${killed} killed` : ""}`;
}

/**
 * A run's attempts, when there was more than one. A fold's `attempts` is the
 * MOST any one iteration took, not a total (Task 3), so it says "up to".
 */
export function attemptsText(s: NodeState): string | null {
  if (s.attempts <= 1) return null;
  return s.iterations ? `up to ${s.attempts} attempts` : `${s.attempts} attempts`;
}

/** `3 s`, or `3 s so far` while it runs; `null` before it started. */
export function durationText(s: NodeState, now: number): string | null {
  if (!s.started_at) return null;
  const start = Date.parse(s.started_at);
  if (s.ended_at) return formatDuration(Date.parse(s.ended_at) - start);
  if (isTerminalState(s.state)) return null;
  return `${formatDuration(now - start)} so far`;
}

/** What a task or trigger card holds besides its title, in the order it is drawn. */
export interface NodeFacts {
  /** Short words: the branch, "Disabled", "Not run". */
  badges: string[];
  refusal: string | null;
  expansion: string | null;
  /** The node's run on an execution graph; `undefined` without states. */
  state: NodeState | null | undefined;
  /** Not run, or disabled: drawn dimmed. */
  dimmed: boolean;
}

export function nodeFacts(
  node: TaskNode | TriggerNode,
  states: Readonly<Record<string, NodeState>> | undefined,
  expansion: ExpansionReason | undefined,
): NodeFacts {
  const badges: string[] = [];
  const branch = node.node === "task" ? BRANCH_LOOK[node.branch] : undefined;
  if (branch) badges.push(branch.word);
  if (node.disabled) badges.push("Disabled");
  // Triggers have no run of their own on an execution, so they are not "not run".
  const state = states && node.node === "task" ? (states[node.uid] ?? null) : undefined;
  if (state === null) badges.push("Not run");
  return {
    badges,
    refusal: node.node === "task" ? (node.refusal ?? null) : null,
    expansion: expansion ? EXPANSION_REASON[expansion] : null,
    state,
    dimmed: node.disabled || state === null,
  };
}

/** Text lines below the title, so the layout can size the card before it is drawn. */
export function factLines(f: NodeFacts): number {
  let lines = f.badges.length > 0 ? 1 : 0;
  if (f.refusal) lines += 2;
  if (f.expansion) lines += 2;
  if (f.state) {
    lines += 1;
    if (f.state.iterations) lines += 1;
    if (attemptsText(f.state)) lines += 1;
  }
  return lines;
}
