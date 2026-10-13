import type { FlowGraph, NodeState } from "@/api/generated/model";

/**
 * What selecting a node of an execution's graph opens (W7c spec §6.3):
 *
 * - `run`: a node with a run of its own in this execution (`run_id`): the
 *   Gantt's run panel for that run.
 * - `iterations`: a node inside a `Loop` body, whose runs live in the loop's
 *   iterations (child executions): the list of the iterations of `loop`, the
 *   OUTERMOST `Loop` around the node, the one this execution ran itself.
 *   `childUid` is the node's uid inside an iteration: an iteration runs the
 *   body as its own `tasks:` (`worker/spawn.rs`, `sub_flow_source`), so
 *   `tasks.loop.body` there is `tasks.body`.
 * - `child`: a node of an expanded `Subflow` (`{subflow}/{rest}`): its runs
 *   are in the Subflow's child execution, which the tab links to.
 * - `none`: the node has no run (it has not run, or not yet), or its runs
 *   cannot be shown here.
 */
export type Selection =
  | { kind: "run"; runId: string }
  | { kind: "iterations"; loop: string; childUid: string }
  | { kind: "child"; subflow: string; rest: string }
  | { kind: "none" };

const CLUSTER = "cluster:";

export function selectionOf(uid: string, graph: FlowGraph, states: Readonly<Record<string, NodeState>>): Selection {
  const slash = uid.indexOf("/");
  if (slash >= 0) return { kind: "child", subflow: uid.slice(0, slash), rest: uid.slice(slash + 1) };
  const runId = states[uid]?.run_id;
  if (runId) return { kind: "run", runId };

  // Walk out from the cluster holding the node; a flowable sits inside its own cluster, which is not its body.
  const byUid = new Map(graph.clusters.map((c) => [c.uid, c]));
  let loop: string | null = null;
  for (let c = graph.clusters.find((x) => x.nodes.includes(uid)); c; c = c.parent ? byUid.get(c.parent) : undefined) {
    const owner = c.uid.slice(CLUSTER.length);
    if (c.kind === "loop" && owner !== uid) loop = owner;
  }
  if (loop !== null) return { kind: "iterations", loop, childUid: `tasks.${uid.slice(loop.length + 1)}` };
  return { kind: "none" };
}

/** The search of a link to `rest` on another execution's Topology tab: expanded through its Subflow when `rest` is inside one. */
export function nodeSearch(rest: string): string {
  const params = new URLSearchParams();
  const slash = rest.indexOf("/");
  if (slash >= 0) params.append("expand", rest.slice(0, slash));
  params.set("node", rest);
  return `?${params.toString()}`;
}
