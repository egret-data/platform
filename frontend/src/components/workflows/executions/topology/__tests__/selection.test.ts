import { describe, expect, it } from "vitest";
import type { FlowGraph } from "@/api/generated/model";
import { nodeSearch, selectionOf } from "@/components/workflows/executions/topology/selection";
import { cluster, end, start, state, task } from "@/components/workflows/topology/__tests__/fixtures";

const LOG = "io.kestra.plugin.core.log.Log";
const LOOP = "io.kestra.plugin.core.flow.ForEach";
const SEQ = "io.kestra.plugin.core.flow.Sequential";

/** A Sequential holding an outer Loop, which holds an inner Loop, which holds `x`. */
const NESTED: FlowGraph = {
  nodes: [
    start("flow"),
    task("tasks.s", SEQ, { kind: "flowable" }),
    task("tasks.s.outer", LOOP, { kind: "flowable" }),
    task("tasks.s.outer.inner", LOOP, { kind: "flowable" }),
    task("tasks.s.outer.inner.x", LOG),
    task("tasks.s.outer.y", LOG),
    task("tasks.s.outer.h", LOG, { branch: "errors" }),
    task("tasks.after", LOG),
    end("flow"),
  ],
  edges: [],
  clusters: [
    cluster("tasks.s", "sequential", ["tasks.s"]),
    cluster("tasks.s.outer", "loop", ["tasks.s.outer", "tasks.s.outer.y", "tasks.s.outer.h"], { parent: "cluster:tasks.s" }),
    cluster("tasks.s.outer.inner", "loop", ["tasks.s.outer.inner", "tasks.s.outer.inner.x"], { parent: "cluster:tasks.s.outer" }),
  ],
};

const folded = state("SUCCESS", { run_id: null, iterations: { total: 0, by_state: {}, runs: [] } });
/** The bodies are folded; `h`, the outer Loop's own `errors:` handler, has not run. */
const STATES = {
  "tasks.s": state("RUNNING"),
  "tasks.s.outer": state("RUNNING", { run_id: "r-outer" }),
  "tasks.s.outer.y": folded,
  "tasks.s.outer.inner": folded,
  "tasks.s.outer.inner.x": folded,
};

describe("selectionOf", () => {
  it("opens the run of a node that has one, a Loop's own node included", () => {
    expect(selectionOf("tasks.s.outer", NESTED, STATES)).toEqual({ kind: "run", runId: "r-outer" });
  });

  it("lists the iterations of the outermost Loop around a body node, naming the node as an iteration runs it", () => {
    expect(selectionOf("tasks.s.outer.y", NESTED, STATES)).toEqual({ kind: "iterations", childUid: "tasks.y" });
    // The inner Loop runs inside the outer one's iterations: its node and its body are the outer Loop's.
    expect(selectionOf("tasks.s.outer.inner", NESTED, STATES)).toEqual({ kind: "iterations", childUid: "tasks.inner" });
    expect(selectionOf("tasks.s.outer.inner.x", NESTED, STATES)).toEqual({
      kind: "iterations",
      childUid: "tasks.inner.x",
    });
  });

  it("sends a node of an expanded Subflow to the child execution, and says when a node has no run", () => {
    expect(selectionOf("tasks.call/tasks.p.x", NESTED, STATES)).toEqual({ kind: "child", subflow: "tasks.call", rest: "tasks.p.x" });
    expect(selectionOf("tasks.after", NESTED, STATES)).toEqual({ kind: "none" });
  });

  it("lists iterations only when the server sent them: an unrun Loop errors handler in the cluster is not run", () => {
    expect(selectionOf("tasks.s.outer.h", NESTED, STATES)).toEqual({ kind: "none" });
    // A body task no iteration has reached yet has no state either.
    expect(selectionOf("tasks.s.outer.y", NESTED, { ...STATES, "tasks.s.outer.y": undefined } as never)).toEqual({ kind: "none" });
  });
});

describe("nodeSearch", () => {
  it("selects the node, and expands the Subflow it sits in", () => {
    expect(nodeSearch("tasks.x")).toBe("?node=tasks.x");
    expect(new URLSearchParams(nodeSearch("tasks.c/tasks.x")).getAll("expand")).toEqual(["tasks.c"]);
    expect(new URLSearchParams(nodeSearch("tasks.c/tasks.x")).get("node")).toBe("tasks.c/tasks.x");
  });
});
