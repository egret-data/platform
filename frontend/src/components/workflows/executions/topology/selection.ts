import type { FlowGraph, NodeState } from "@/api/generated/model";

/**
 * What selecting a node of an execution's graph opens (W7c spec §6.3):
 *
 * - `run`: a node with a run of its own in this execution (`run_id`): the
 *   Gantt's run panel for that run.
 * - `iterations`: a node whose runs live in `Loop` iterations, which the
 *   server says by sending `iterations` (it knows body membership; a Loop's
 *   own `errors:` handler sits in the loop's cluster too, but runs in the
 *   execution itself): the node's own run in each iteration. `childUid` is the
 *   node's uid inside an iteration, null when no `Loop` cluster holds it: an
 *   iteration runs the outermost Loop's body as its own `tasks:`
 *   (`worker/spawn.rs`, `sub_flow_source`), so `tasks.loop.body` there is
 *   `tasks.body`.
 * - `child`: a node of an expanded `Subflow` (`{subflow}/{rest}`): its runs
 *   are in the Subflow's child execution, which the tab links to.
 * - `none`: the node has no run (it has not run, or not yet).
 */
export type Selection =
  | { kind: "run"; runId: string }
  | { kind: "iterations"; childUid: string | null }
  | { kind: "child"; subflow: string; rest: string }
  | { kind: "none" };

const CLUSTER = "cluster:";

export function selectionOf(uid: string, graph: FlowGraph, states: Readonly<Record<string, NodeState>>): Selection {
  const slash = uid.indexOf("/");
  if (slash >= 0) return { kind: "child", subflow: uid.slice(0, slash), rest: uid.slice(slash + 1) };
  const state = states[uid];
  if (state?.run_id) return { kind: "run", runId: state.run_id };
  if (!state?.iterations) return { kind: "none" };

  // Walk out from the cluster holding the node to the outermost Loop; a flowable sits inside its own cluster, which is not its body.
  const byUid = new Map(graph.clusters.map((c) => [c.uid, c]));
  let loop: string | null = null;
  for (let c = graph.clusters.find((x) => x.nodes.includes(uid)); c; c = c.parent ? byUid.get(c.parent) : undefined) {
    const owner = c.uid.slice(CLUSTER.length);
    if (c.kind === "loop" && owner !== uid) loop = owner;
  }
  return { kind: "iterations", childUid: loop === null ? null : `tasks.${uid.slice(loop.length + 1)}` };
}

/** The search of a link to `rest` on another execution's Topology tab: expanded through its Subflow when `rest` is inside one. */
export function nodeSearch(rest: string): string {
  const params = new URLSearchParams();
  const slash = rest.indexOf("/");
  if (slash >= 0) params.append("expand", rest.slice(0, slash));
  params.set("node", rest);
  return `?${params.toString()}`;
}
