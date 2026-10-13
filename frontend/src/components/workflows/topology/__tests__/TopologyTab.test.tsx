import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ApiError } from "@/api/client";
import type { FlowGraph } from "@/api/generated/model";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { rangeOfPath } from "@/components/workflows/editor/yamlPath";
import { TOPOLOGY_DEBOUNCE_MS, TopologyTab } from "@/components/workflows/topology/TopologyTab";
import { STALE_BANNER } from "@/components/workflows/topology/TopologyView";
import { FlowEditPage, useFlowLayout } from "@/routes/workflows/FlowEditPage";
import { makeEditorContext } from "@/components/workflows/editor/__tests__/fixtures";
import { installXyflowJsdom } from "./xyflowJsdom";
import { EXPANDED, edge, end, start, task } from "./fixtures";

const unsaved = vi.fn();
const saved = vi.fn();
vi.mock("@/api/generated/workflow/workflow", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/generated/workflow/workflow")>()),
  workflowGraphUnsavedFlow: (...a: unknown[]) => unsaved(...a),
  workflowGetFlowGraph: (...a: unknown[]) => saved(...a),
}));
vi.mock("@/components/monaco/MonacoEditor", () => import("@/routes/workflows/__tests__/monacoStub"));
vi.mock("@/components/workflows/editor/monacoYaml", () => ({ configureFlowYaml: vi.fn() }));

const LOG = "io.kestra.plugin.core.log.Log";
const SUBFLOW = "io.kestra.plugin.core.flow.Subflow";

const SOURCE = [
  "id: hello",
  "namespace: company.team",
  "tasks:",
  "  - id: first",
  "    type: io.kestra.plugin.core.log.Log",
  "    message: hi",
  "  - id: call",
  "    type: io.kestra.plugin.core.flow.Subflow",
  "    namespace: company.team",
  "    flowId: child",
  "",
].join("\n");

/** `SOURCE`'s graph, its `Subflow` not expanded. Nodes out of drawing order (FE-TEST-1). */
const GRAPH: FlowGraph = {
  nodes: [task("tasks.call", SUBFLOW, { kind: "executable" }), end("flow"), task("tasks.first", LOG), start("flow")],
  edges: [edge("tasks.first", "tasks.call"), edge("start:flow", "tasks.first"), edge("tasks.call", "end:flow")],
  clusters: [],
};

/** After an edit that added `tasks.second`. */
const GRAPH_2: FlowGraph = {
  nodes: [...GRAPH.nodes, task("tasks.second", LOG)],
  edges: [edge("start:flow", "tasks.first"), edge("tasks.first", "tasks.second"), edge("tasks.second", "tasks.call"), edge("tasks.call", "end:flow")],
  clusters: [],
};

const ok = (graph: FlowGraph) => ({ status: 200, data: graph, headers: new Headers() });
const parseError = () => new ApiError(400, "invalid flow YAML: did not find expected key (line 4, column 3)", "invalid_yaml");

/** The node button carrying `uid`, or null. */
const nodeButton = (uid: string) =>
  screen.queryAllByRole("button").find((b) => b.getAttribute("data-uid") === uid) ?? null;

/** Lets the request's promise, React's commit and xyflow's measuring (microtasks) run. */
const flush = () => act(async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
});
const advance = (ms: number) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

/**
 * The tab remembers the last flow's graph across a remount (by workspace and flow id), so each
 * render gets a flow id of its own: no test starts from another test's graph.
 */
let flows = 0;

function renderTab(over: Partial<EditorContext> = {}) {
  const ctx = makeEditorContext({ source: SOURCE, modelKey: `flow-${++flows}`, ...over });
  const utils = render(
    <div style={{ width: 800, height: 600 }}>
      <TopologyTab ctx={ctx} />
    </div>,
  );
  const edit = (source: string) => {
    const next = { ...ctx, source };
    utils.rerender(
      <div style={{ width: 800, height: 600 }}>
        <TopologyTab ctx={next} />
      </div>,
    );
  };
  return { ...utils, ctx, edit };
}

beforeAll(installXyflowJsdom);

beforeEach(() => {
  unsaved.mockReset();
  saved.mockReset();
  try {
    localStorage.clear();
  } catch {
    /* no storage in this environment */
  }
});
afterEach(() => vi.useRealTimers());

describe("TopologyTab: updates (spec §6.2)", () => {
  beforeEach(() => vi.useFakeTimers());

  it("draws the YAML at once when opened, then waits 1 s after the last edit, sending the current source", async () => {
    unsaved.mockResolvedValue(ok(GRAPH));
    const { edit } = renderTab();
    await flush();
    expect(unsaved).toHaveBeenCalledTimes(1);
    expect(unsaved.mock.calls[0][0]).toBe("ws1");
    expect(unsaved.mock.calls[0][1]).toEqual({ source: SOURCE });

    expect(TOPOLOGY_DEBOUNCE_MS).toBe(1000);
    edit(`${SOURCE}# one`);
    await advance(600);
    edit(`${SOURCE}# two`);
    await advance(600);
    edit(`${SOURCE}# three`);
    await advance(999);
    expect(unsaved).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(unsaved).toHaveBeenCalledTimes(2);
    expect(unsaved.mock.calls[1][1]).toEqual({ source: `${SOURCE}# three` });
    await advance(5000);
    expect(unsaved).toHaveBeenCalledTimes(2);
  });

  it("draws the new graph the answer brings", async () => {
    unsaved.mockResolvedValueOnce(ok(GRAPH)).mockResolvedValueOnce(ok(GRAPH_2));
    const { edit } = renderTab();
    await flush();
    expect(nodeButton("tasks.first")).not.toBeNull();
    expect(nodeButton("tasks.second")).toBeNull();
    edit(`${SOURCE}# edited`);
    await advance(1000);
    await flush();
    expect(nodeButton("tasks.second")).not.toBeNull();
  });

  it("aborts a request an edit made obsolete, and ignores its answer", async () => {
    let resolveFirst!: (v: unknown) => void;
    unsaved.mockImplementationOnce(() => new Promise((r) => { resolveFirst = r; })).mockResolvedValueOnce(ok(GRAPH_2));
    const { edit } = renderTab();
    await flush();
    edit(`${SOURCE}# edited`);
    const firstSignal = (unsaved.mock.calls[0].at(-1) as { signal: AbortSignal }).signal;
    expect(firstSignal.aborted).toBe(true);
    await advance(1000);
    await flush();
    await act(async () => { resolveFirst(ok(GRAPH)); });
    await flush();
    expect(nodeButton("tasks.second")).not.toBeNull();
  });

  it("keeps the last graph on a YAML parse error (400) and says it is out of date", async () => {
    unsaved.mockResolvedValueOnce(ok(GRAPH)).mockRejectedValueOnce(parseError()).mockResolvedValueOnce(ok(GRAPH_2));
    const { edit } = renderTab();
    await flush();
    expect(screen.queryByText(STALE_BANNER)).toBeNull();

    edit("tasks: [");
    await advance(1000);
    await flush();
    expect(STALE_BANNER).toBe("The graph is out of date: the YAML has an error");
    expect(screen.getByText(STALE_BANNER)).toBeInTheDocument();
    expect(nodeButton("tasks.first")).not.toBeNull();
    expect(nodeButton("tasks.call")).not.toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();

    // Fixed: the banner goes with the next good answer.
    edit(`${SOURCE}# fixed`);
    await advance(1000);
    await flush();
    expect(screen.queryByText(STALE_BANNER)).toBeNull();
    expect(nodeButton("tasks.second")).not.toBeNull();
  });

  it("with no graph to keep, a parse error says where the YAML is wrong, not that a graph is out of date", async () => {
    unsaved.mockRejectedValueOnce(parseError());
    renderTab({ source: "tasks: [" });
    await flush();
    expect(screen.queryByText(STALE_BANNER)).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent("line 4, column 3");
  });

  it("keeps the last graph when the server refuses it, and shows why (graph_too_large)", async () => {
    unsaved
      .mockResolvedValueOnce(ok(GRAPH))
      .mockRejectedValueOnce(new ApiError(422, "the graph has 5,001 nodes; the limit is 5,000", "graph_too_large"));
    const { edit } = renderTab();
    await flush();
    edit(`${SOURCE}# huge`);
    await advance(1000);
    await flush();
    expect(screen.getByRole("alert")).toHaveTextContent("the limit is 5,000");
    expect(nodeButton("tasks.first")).not.toBeNull();
  });

  it("expanding a Subflow refetches at once with `subflows`, and the expansion survives an edit", async () => {
    unsaved.mockResolvedValueOnce(ok(GRAPH)).mockResolvedValueOnce(ok(EXPANDED)).mockResolvedValue(ok(EXPANDED));
    const { edit } = renderTab();
    await flush();
    expect(unsaved.mock.calls[0][2]).toEqual({ subflows: [] });

    fireEvent.click(screen.getByRole("button", { name: "Expand subflow call" }));
    await flush();
    expect(unsaved).toHaveBeenCalledTimes(2);
    expect(unsaved.mock.calls[1][1]).toEqual({ source: SOURCE });
    expect(unsaved.mock.calls[1][2]).toEqual({ subflows: ["tasks.call"] });
    expect(nodeButton("tasks.call/tasks.p.x")).not.toBeNull();

    edit(`${SOURCE}# edited`);
    await advance(1000);
    expect(unsaved).toHaveBeenCalledTimes(3);
    expect(unsaved.mock.calls[2][1]).toEqual({ source: `${SOURCE}# edited` });
    expect(unsaved.mock.calls[2][2]).toEqual({ subflows: ["tasks.call"] });
  });

  it("a read-only viewer's tab draws the saved draft through the read route", async () => {
    saved.mockResolvedValue(ok(GRAPH));
    const { ctx } = renderTab({ readOnly: true });
    await flush();
    expect(unsaved).not.toHaveBeenCalled();
    expect(saved).toHaveBeenCalledTimes(1);
    expect(saved.mock.calls[0].slice(0, 3)).toEqual(["ws1", ctx.modelKey, { subflows: [] }]);
    expect(nodeButton("tasks.first")).not.toBeNull();
  });
});

describe("TopologyTab: selection (spec §6.2)", () => {
  it("selecting a task moves the Code tab's cursor to it, through the Problems tab's path-to-range mapping", async () => {
    unsaved.mockResolvedValue(ok(GRAPH));
    const { ctx } = renderTab();
    await waitFor(() => expect(nodeButton("tasks.call")).not.toBeNull());
    await userEvent.click(nodeButton("tasks.call") as HTMLElement);
    const range = rangeOfPath(SOURCE, "tasks.call");
    expect(range).not.toBeNull();
    expect(range?.startLine).toBe(7);
    expect(ctx.focusRange).toHaveBeenCalledTimes(1);
    expect(ctx.focusRange).toHaveBeenCalledWith(range);
  });

  it("a task of an expanded Subflow belongs to another flow: the cursor stays, and the tab says why", async () => {
    unsaved.mockResolvedValue(ok(EXPANDED));
    const { ctx } = renderTab();
    await waitFor(() => expect(nodeButton("tasks.call/tasks.p.x")).not.toBeNull());
    const child = nodeButton("tasks.call/tasks.p.x");
    expect(child).not.toBeNull();
    await userEvent.click(child as HTMLElement);
    expect(ctx.focusRange).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("x is a task of the Subflow call's flow");
  });
});

describe("TopologyTab: registration (spec §6.2, W7a §5.1)", () => {
  function Page({ flowKey, ctx }: { flowKey: string; ctx: EditorContext }) {
    const flowLayout = useFlowLayout(flowKey);
    return (
      <div style={{ height: 600 }}>
        <FlowEditPage ctx={ctx} flowLayout={flowLayout} />
      </div>
    );
  }

  it("Topology is a tab of the flow editor, offered in the panel's tab menu", async () => {
    const user = userEvent.setup();
    render(<Page flowKey="flow-menu" ctx={makeEditorContext({ source: SOURCE, modelKey: "flow-menu" })} />);
    screen.getByRole("button", { name: "Reopen a tab in panel 2" }).focus();
    await user.keyboard("{Enter}");
    const item = await screen.findByRole("menuitem", { name: /Topology/ });
    await user.click(item);
    const strip = screen.getByRole("tablist", { name: "Panel 2" });
    expect(within(strip).getByRole("tab", { name: "Topology" })).toHaveAttribute("aria-selected", "true");
  });

  it("a stored layout that names topology keeps it, and its body loads", async () => {
    unsaved.mockResolvedValue(ok(GRAPH));
    localStorage.setItem(
      "egret.wf.layout.flow-stored",
      JSON.stringify({
        panels: [
          { id: "p1", size: 50, tabs: ["code"], active: "code" },
          { id: "p2", size: 50, tabs: ["docs", "topology"], active: "topology" },
        ],
      }),
    );
    render(<Page flowKey="flow-stored" ctx={makeEditorContext({ source: SOURCE, modelKey: "flow-stored" })} />);
    const strip = screen.getByRole("tablist", { name: "Panel 2" });
    expect(within(strip).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Docs", "Topology"]);
    expect(within(strip).getByRole("tab", { name: "Topology" })).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: "Topology" });
    await waitFor(() => expect(within(panel).queryAllByRole("button").some((b) => b.getAttribute("data-uid") === "tasks.first")).toBe(true));
  });
});
