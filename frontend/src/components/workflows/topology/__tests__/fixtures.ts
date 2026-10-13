import type { Cluster, Edge, FlowGraph, Node, NodeState, Relation } from "@/api/generated/model";

/** A task node; `canonical_type` follows `type` unless given. */
export function task(uid: string, type: string, over: Partial<Extract<Node, { node: "task" }>> = {}): Node {
  return {
    uid,
    node: "task",
    task_id: uid.split(/[./]/).pop() ?? uid,
    type,
    canonical_type: type,
    kind: "leaf",
    branch: "tasks",
    disabled: false,
    refusal: null,
    ...over,
  };
}

export const trigger = (uid: string, type: string): Node => ({ uid, node: "trigger", trigger_id: uid.split(".").pop() ?? uid, type, disabled: false });
export const start = (owner: string): Node => ({ uid: `start:${owner}`, node: "start" });
export const end = (owner: string): Node => ({ uid: `end:${owner}`, node: "end" });

export function edge(source: string, target: string, relation: Relation = "sequential", label: string | null = null): Edge {
  return { source, target, relation, label };
}

export function cluster(owner: string, kind: Cluster["kind"], nodes: string[], over: Partial<Cluster> = {}): Cluster {
  return {
    uid: `cluster:${owner}`,
    task_id: owner === "triggers" ? null : (owner.split(/[./]/).pop() ?? owner),
    kind,
    nodes,
    parent: null,
    start: `start:${owner}`,
    end: `end:${owner}`,
    reason: null,
    ...over,
  };
}

const PARALLEL = "io.kestra.plugin.core.flow.Parallel";
const IF = "io.kestra.plugin.core.flow.If";
const SUBFLOW = "io.kestra.plugin.core.flow.Subflow";
const LOG = "io.kestra.plugin.core.log.Log";

/**
 * A flow with a trigger, a `Parallel`, an `If`, a disabled task, a `Subflow`
 * whose expansion failed (`not_found`), one that was never expanded, a refused
 * task, an `errors:` branch and a `finally:` branch.
 *
 * The nodes are NOT in drawing order (FE-TEST-1): the layout, not the array,
 * must decide what comes first.
 */
export const FLOW: FlowGraph = {
  nodes: [
    task("finally.cleanup", LOG, { branch: "finally" }),
    task("tasks.p.y", LOG),
    end("flow"),
    task("tasks.check.no", LOG),
    task("errors.alert", LOG, { branch: "errors" }),
    task("tasks.bad", "io.kestra.plugin.scripts.python.Script", { refusal: "unsupported_task_type" }),
    task("tasks.call", "io.kestra.plugin.core.flow.Subflow", { kind: "executable", canonical_type: SUBFLOW }),
    task("tasks.call2", "Subflow", { kind: "executable", canonical_type: SUBFLOW }),
    task("tasks.b", LOG, { disabled: true }),
    start("tasks.check"),
    task("tasks.check", IF, { kind: "flowable" }),
    task("tasks.check.yes", LOG),
    end("tasks.check"),
    task("tasks.p.x", LOG),
    end("tasks.p"),
    task("tasks.p", PARALLEL, { kind: "flowable" }),
    start("tasks.p"),
    task("tasks.a", LOG),
    start("flow"),
    end("triggers"),
    trigger("triggers.daily", "io.kestra.plugin.core.trigger.Schedule"),
    start("triggers"),
  ],
  edges: [
    edge("finally.cleanup", "end:flow", "finally"),
    edge("errors.alert", "finally.cleanup", "error"),
    edge("start:flow", "errors.alert", "error"),
    edge("tasks.bad", "finally.cleanup", "finally"),
    edge("tasks.call2", "tasks.bad"),
    edge("tasks.call", "tasks.call2"),
    edge("tasks.b", "tasks.call"),
    edge("end:tasks.check", "tasks.b"),
    edge("tasks.check.no", "end:tasks.check", "sequential", "else"),
    edge("tasks.check.yes", "end:tasks.check", "sequential", "then"),
    edge("tasks.check", "tasks.check.no", "choice", "else"),
    edge("tasks.check", "tasks.check.yes", "choice", "then"),
    edge("start:tasks.check", "tasks.check"),
    edge("end:tasks.p", "start:tasks.check"),
    edge("tasks.p.y", "end:tasks.p", "parallel"),
    edge("tasks.p.x", "end:tasks.p", "parallel"),
    edge("tasks.p", "tasks.p.y", "parallel"),
    edge("tasks.p", "tasks.p.x", "parallel"),
    edge("start:tasks.p", "tasks.p"),
    edge("tasks.a", "start:tasks.p"),
    edge("start:flow", "tasks.a"),
    edge("end:triggers", "start:flow"),
    edge("triggers.daily", "end:triggers"),
    edge("start:triggers", "triggers.daily"),
  ],
  clusters: [
    cluster("tasks.check", "if", ["start:tasks.check", "tasks.check", "tasks.check.yes", "tasks.check.no", "end:tasks.check"]),
    // A failed expansion: no nodes, no markers; `start`/`end` are the Subflow node's own uid.
    cluster("tasks.call", "subflow", [], { start: "tasks.call", end: "tasks.call", reason: "not_found" }),
    cluster("tasks.p", "parallel", ["start:tasks.p", "tasks.p", "tasks.p.x", "tasks.p.y", "end:tasks.p"]),
    cluster("triggers", "triggers", ["start:triggers", "triggers.daily", "end:triggers"]),
  ],
};

/** A `Sequential` holding a `Parallel`, to collapse the outer one. */
export const NESTED: FlowGraph = {
  nodes: [
    task("tasks.s.p.z", LOG),
    start("flow"),
    start("tasks.s"),
    task("tasks.s", "io.kestra.plugin.core.flow.Sequential", { kind: "flowable" }),
    start("tasks.s.p"),
    task("tasks.s.p", PARALLEL, { kind: "flowable" }),
    end("tasks.s.p"),
    end("tasks.s"),
    task("tasks.after", LOG),
    end("flow"),
  ],
  edges: [
    edge("start:flow", "start:tasks.s"),
    edge("start:tasks.s", "tasks.s"),
    edge("tasks.s", "start:tasks.s.p"),
    edge("start:tasks.s.p", "tasks.s.p"),
    edge("tasks.s.p", "tasks.s.p.z", "parallel"),
    edge("tasks.s.p.z", "end:tasks.s.p", "parallel"),
    edge("end:tasks.s.p", "end:tasks.s"),
    edge("end:tasks.s", "tasks.after"),
    edge("tasks.after", "end:flow"),
  ],
  clusters: [
    cluster("tasks.s.p", "parallel", ["start:tasks.s.p", "tasks.s.p", "tasks.s.p.z", "end:tasks.s.p"], { parent: "cluster:tasks.s" }),
    cluster("tasks.s", "sequential", ["start:tasks.s", "tasks.s", "end:tasks.s"]),
  ],
};

/** start → one → two → three → end, with the nodes listed out of order. */
export const LINE: FlowGraph = {
  nodes: [task("tasks.three", LOG), end("flow"), task("tasks.one", LOG), start("flow"), task("tasks.two", LOG)],
  edges: [edge("tasks.two", "tasks.three"), edge("start:flow", "tasks.one"), edge("tasks.three", "end:flow"), edge("tasks.one", "tasks.two")],
  clusters: [],
};

export const EMPTY_FLOW: FlowGraph = {
  nodes: [start("flow"), end("flow")],
  edges: [edge("start:flow", "end:flow")],
  clusters: [],
};

const T0 = Date.parse("2026-10-13T10:00:00Z");
const at = (s: number) => new Date(T0 + s * 1000).toISOString();

export function state(s: string, over: Partial<NodeState> = {}): NodeState {
  return { state: s, attempts: 1, run_id: `run-${s}`, started_at: at(0), ended_at: at(3), ...over };
}

/** `tasks.p.y` is a folded `Loop` body: three iterations, the worst run took three attempts. */
export const STATES: Record<string, NodeState> = {
  "tasks.a": state("SUCCESS"),
  "tasks.p": state("FAILED", { ended_at: at(90) }),
  "tasks.p.x": state("FAILED", { attempts: 2, ended_at: at(65) }),
  "tasks.p.y": state("FAILED", {
    attempts: 3,
    run_id: null,
    iterations: { total: 3, by_state: { SUCCESS: 2, FAILED: 1 } },
  }),
};
