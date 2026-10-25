import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { READABLE_ZOOM, TopologyView, type TopologyViewProps } from "@/components/workflows/topology/TopologyView";
import { installXyflowJsdom, setXyflowSize } from "./xyflowJsdom";
import { EMPTY_FLOW, EXPANDED, FLOW, LINE, NESTED, NESTED_FAILED, STATES, chain } from "./fixtures";

const ORIENTATION_KEY = "egret.workflows.topology.orientation";

function renderView(over: Partial<TopologyViewProps> = {}) {
  const props: TopologyViewProps = { graph: FLOW, onSelect: vi.fn(), onExpandSubflow: vi.fn(), ...over };
  const utils = render(
    <div style={{ width: 800, height: 600 }}>
      <TopologyView {...props} />
    </div>,
  );
  return { ...utils, props };
}

/** A node's main button, found by the uid it carries. */
function node(uid: string): HTMLElement {
  const found = screen.queryAllByRole("button").find((b) => b.getAttribute("data-uid") === uid);
  if (!found) throw new Error(`no node button for ${uid}`);
  return found;
}
const hasNode = (uid: string) => screen.queryAllByRole("button").some((b) => b.getAttribute("data-uid") === uid);
/** The card around a node's button: where dimming, branch and notes live. */
const card = (uid: string) => node(uid).closest("[data-node-card]") as HTMLElement;

/** Lets xyflow's measuring (the observer above) and its queued fit run. */
const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

function viewportScale(container: HTMLElement): number {
  const vp = container.querySelector(".react-flow__viewport") as HTMLElement;
  const m = /scale\(([\d.]+)\)/.exec(vp.style.transform);
  if (!m) throw new Error(`no scale in ${vp.style.transform}`);
  return Number(m[1]);
}

beforeAll(installXyflowJsdom);

beforeEach(() => {
  try {
    localStorage.clear();
  } catch {
    /* no storage in this environment */
  }
});
afterEach(() => vi.useRealTimers());

describe("TopologyView nodes", () => {
  it("shows a task's type icon, task id and short type", () => {
    renderView();
    const a = node("tasks.a");
    expect(within(a).getByText("a")).toBeInTheDocument();
    expect(within(a).getByText("Log")).toBeInTheDocument();
    expect(a.querySelector("svg[data-icon='log']")).not.toBeNull();
    // A flowable's icon comes from its construct, not from a generic task glyph.
    expect(node("tasks.p").querySelector("svg[data-icon='parallel']")).not.toBeNull();
    expect(within(node("tasks.p")).getByText("Parallel")).toBeInTheDocument();
    expect(node("tasks.call").querySelector("svg[data-icon='subflow']")).not.toBeNull();
  });

  it("with states, shows the state chip, the duration, retries and a Loop body's iterations", () => {
    renderView({ states: STATES });
    const a = node("tasks.a");
    expect(within(a).getByText("Success")).toHaveAttribute("data-state", "SUCCESS");
    expect(within(a).getByText("3 s")).toBeInTheDocument();
    expect(within(node("tasks.p.x")).getByText("1 min 5 s")).toBeInTheDocument();
    expect(within(node("tasks.p.x")).getByText("2 attempts")).toBeInTheDocument();
    const loopBody = node("tasks.p.y");
    expect(within(loopBody).getByText("2 of 3 iterations done, 1 failed")).toBeInTheDocument();
    // A fold's attempt count is the most any one iteration took, never a sum.
    expect(within(loopBody).getByText("up to 3 attempts")).toBeInTheDocument();
    expect(within(loopBody).queryByText(/^\d+ attempts$/)).toBeNull();
  });

  it("dims a task with no state on an execution, and says it did not run", () => {
    renderView({ states: STATES });
    expect(card("tasks.check.yes")).toHaveAttribute("data-dimmed", "true");
    expect(within(node("tasks.check.yes")).getByText("Not run")).toBeInTheDocument();
    expect(card("tasks.a")).not.toHaveAttribute("data-dimmed");
    // Without states (the editor's graph) nothing is dimmed for want of a run.
    cleanup();
    renderView();
    expect(screen.queryAllByText("Not run")).toHaveLength(0);
    expect(card("tasks.check.yes")).not.toHaveAttribute("data-dimmed");
  });

  it("says a task with no run has not started while the execution runs (KI-191)", () => {
    renderView({ states: STATES, running: true });
    expect(within(node("tasks.check.yes")).getByText("Not started")).toBeInTheDocument();
    expect(screen.queryAllByText("Not run")).toHaveLength(0);
  });

  it("says Not started in the list view too while the execution runs (KI-191)", async () => {
    const user = userEvent.setup();
    renderView({ states: STATES, running: true });
    await user.click(screen.getByRole("button", { name: "List view" }));
    const grid = screen.getByRole("treegrid", { name: "Tasks of the flow" });
    const row = within(grid).getAllByRole("row").find((el) => el.getAttribute("data-uid") === "tasks.check.yes")!;
    expect(within(row).getByText("Not started")).toBeInTheDocument();
    expect(within(row).queryByText("Not run")).toBeNull();
  });

  it("marks a refused task with the note and its code", () => {
    renderView();
    const bad = card("tasks.bad");
    expect(within(bad).getByText("egret does not run this task")).toBeInTheDocument();
    expect(within(bad).getByText("unsupported_task_type")).toBeInTheDocument();
    expect(bad).toHaveAttribute("data-refused", "true");
    expect(card("tasks.a")).not.toHaveAttribute("data-refused");
  });

  it("dims a disabled task and labels it", () => {
    renderView();
    expect(card("tasks.b")).toHaveAttribute("data-dimmed", "true");
    expect(within(node("tasks.b")).getByText("Disabled")).toBeInTheDocument();
    expect(within(node("tasks.a")).queryByText("Disabled")).toBeNull();
  });

  it("says why a Subflow did not expand, on its node", () => {
    renderView();
    expect(within(card("tasks.call")).getByText("No flow by that namespace and id in this workspace")).toBeInTheDocument();
    expect(within(card("tasks.call")).queryByRole("button", { name: /^Expand subflow/ })).toBeNull();
  });

  it("says why a Subflow nested in a cluster did not expand, with no box of its own", () => {
    renderView({ graph: NESTED_FAILED });
    expect(within(card("tasks.s.call")).getByText("Not expanded: Subflows expand two levels deep")).toBeInTheDocument();
    expect(within(card("tasks.s.call")).queryByRole("button", { name: /^Expand subflow/ })).toBeNull();
    // Drawn as a child of the Sequential's box (xyflow's `parentId`), not of a box of its own.
    expect(document.querySelector('[data-cluster="cluster:tasks.s.call"]')).toBeNull();
  });

  it("names a node's branch in words, not only in colour", () => {
    renderView();
    expect(within(card("errors.alert")).getByText("On error")).toBeInTheDocument();
    expect(within(card("finally.cleanup")).getByText("Finally")).toBeInTheDocument();
    expect(within(card("tasks.a")).queryByText(/On error|Finally/)).toBeNull();
  });
});

describe("TopologyView edges and colours", () => {
  const path = (container: HTMLElement, source: string, target: string) => {
    const p = container.querySelector(`path[data-edge="${source} -> ${target}"]`) as SVGPathElement | null;
    if (!p) throw new Error(`no edge ${source} -> ${target}`);
    return p;
  };

  it("colours branches with tokens: errors in the danger token, finally in the warning token", () => {
    const { container } = renderView();
    expect(path(container, "start:flow", "errors.alert").style.stroke).toBe("var(--destructive)");
    expect(path(container, "finally.cleanup", "end:flow").style.stroke).toBe("var(--highlight)");
    expect(path(container, "start:flow", "tasks.a").style.stroke).toBe("var(--muted-foreground)");
    expect(card("errors.alert").style.borderColor).toBe("var(--destructive)");
  });

  it("uses no literal colour anywhere it draws", () => {
    const { container } = renderView({ states: STATES });
    const literal = /#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(|oklch\(/i;
    const offenders = [...container.querySelectorAll("[style]")]
      .map((el) => el.getAttribute("style") ?? "")
      .filter((s) => literal.test(s));
    expect(offenders).toEqual([]);
    for (const el of container.querySelectorAll("[fill],[stroke]")) {
      expect(literal.test(el.getAttribute("fill") ?? "")).toBe(false);
      expect(literal.test(el.getAttribute("stroke") ?? "")).toBe(false);
    }
  });

  it("paints routes above cluster boxes and beneath every card", () => {
    const { container } = renderView({ graph: NESTED });
    const z = (el: Element | null) => Number((el as HTMLElement).style.zIndex);
    const edgeZ = z(container.querySelector("[data-edge-layer]"));
    expect(z(container.querySelector("[data-edge-labels]"))).toBe(edgeZ);
    const wrappers = [...container.querySelectorAll(".react-flow__node")] as HTMLElement[];
    const boxes = wrappers.filter((w) => w.querySelector("[data-cluster]"));
    const cards = wrappers.filter((w) => !w.querySelector("[data-cluster]"));
    expect(boxes).toHaveLength(2);
    expect(cards.length).toBeGreaterThan(4);
    for (const b of boxes) expect(z(b)).toBeLessThan(edgeZ);
    for (const c of cards) expect(z(c)).toBeGreaterThan(edgeZ);
  });

  it("dashes the trigger chain inside an expanded Subflow", () => {
    const { container } = renderView({ graph: EXPANDED });
    expect(path(container, "tasks.call/triggers.hook", "end:tasks.call/triggers").style.strokeDasharray).not.toBe("");
    expect(path(container, "tasks.call", "start:tasks.call/tasks.p").style.strokeDasharray).toBe("");
  });

  it("labels choice edges and dashes trigger edges", () => {
    const { container } = renderView();
    const labels = within(container.querySelector("[data-edge-labels]") as HTMLElement);
    expect(labels.getByText("then")).toBeInTheDocument();
    expect(labels.getByText("else")).toBeInTheDocument();
    expect(path(container, "triggers.daily", "end:triggers").style.strokeDasharray).not.toBe("");
    expect(path(container, "end:triggers", "start:flow").style.strokeDasharray).not.toBe("");
    expect(path(container, "start:flow", "tasks.a").style.strokeDasharray).toBe("");
  });
});

/** The viewport's pan and zoom, as xyflow writes them on the viewport element. */
function viewportOf(container: HTMLElement): { x: number; y: number; zoom: number } {
  const t = (container.querySelector(".react-flow__viewport") as HTMLElement).style.transform;
  const m = /translate\(([-\d.e]+)px,\s*([-\d.e]+)px\)\s*scale\(([\d.e]+)\)/.exec(t);
  if (!m) throw new Error(`no viewport transform in ${t}`);
  return { x: Number(m[1]), y: Number(m[2]), zoom: Number(m[3]) };
}

/** A node's box on the screen: its graph position through the viewport (the 800 × 600 canvas of `installXyflowJsdom`). */
function onScreen(container: HTMLElement, uid: string): { left: number; top: number; right: number; bottom: number } {
  const el = container.querySelector(`.react-flow__node[data-id="${uid}"]`) as HTMLElement;
  const m = /translate\(([-\d.e]+)px,\s*([-\d.e]+)px\)/.exec(el.style.transform)!;
  const v = viewportOf(container);
  const left = Number(m[1]) * v.zoom + v.x;
  const top = Number(m[2]) * v.zoom + v.y;
  return { left, top, right: left + Number.parseFloat(el.style.width || "10") * v.zoom, bottom: top + Number.parseFloat(el.style.height || "10") * v.zoom };
}
const inCanvas = (b: { left: number; top: number; right: number; bottom: number }, width = 800, height = 600) =>
  b.left >= 0 && b.top >= 0 && b.right <= width && b.bottom <= height;

describe("TopologyView first view", () => {
  it("a graph too long to fit readably opens at the readable zoom with the flow's start in view", async () => {
    const { container } = renderView({ graph: chain(30) });
    await settle();
    expect(viewportOf(container).zoom).toBeCloseTo(READABLE_ZOOM, 5);
    expect(inCanvas(onScreen(container, "start:flow"))).toBe(true);
    // The first rank is near the leading (left) edge, and the first task is in view too.
    expect(onScreen(container, "start:flow").left).toBeLessThan(100);
    expect(inCanvas(onScreen(container, "tasks.t0"))).toBe(true);
    // Fit still fits the whole graph, below the readable zoom.
    await userEvent.setup().click(screen.getByRole("button", { name: "Fit view" }));
    await settle();
    expect(viewportOf(container).zoom).toBeLessThan(READABLE_ZOOM);
    expect(inCanvas(onScreen(container, `tasks.t29`))).toBe(true);
  });

  it("vertically, the start is near the top", async () => {
    localStorage.setItem(ORIENTATION_KEY, "TB");
    const { container } = renderView({ graph: chain(30) });
    await settle();
    expect(viewportOf(container).zoom).toBeCloseTo(READABLE_ZOOM, 5);
    const startBox = onScreen(container, "start:flow");
    expect(inCanvas(startBox)).toBe(true);
    expect(startBox.top).toBeLessThan(100);
  });

  // (The Fit button is not compared: jsdom's observer reports every node at 800 × 600, which xyflow's own fit measures.)
  // The editor's default panel is about 480 × 370; a flow with a trigger chain before its start.
  for (const orientation of ["LR", "TB"] as const) {
    it(`in a narrow canvas, a flow with triggers opens on its start and its first task (${orientation})`, async () => {
      setXyflowSize(480, 370);
      try {
        localStorage.setItem(ORIENTATION_KEY, orientation);
        const { container } = renderView({ graph: FLOW });
        await settle();
        expect(viewportOf(container).zoom).toBeCloseTo(READABLE_ZOOM, 5);
        expect(inCanvas(onScreen(container, "start:flow"), 480, 370), "start:flow").toBe(true);
        expect(inCanvas(onScreen(container, "tasks.a"), 480, 370), "the first task").toBe(true);
      } finally {
        setXyflowSize(800, 600);
      }
    });
  }

  it("a small graph opens fitted: the whole graph in view, at a readable zoom, no larger than life", async () => {
    // Two tasks fit the 800 px canvas above the readable zoom.
    const { container } = renderView({ graph: chain(2) });
    await settle();
    const first = viewportOf(container);
    expect(first.zoom).toBeGreaterThanOrEqual(READABLE_ZOOM);
    expect(first.zoom).toBeLessThanOrEqual(1);
    for (const uid of ["start:flow", "tasks.t0", "tasks.t1", "end:flow"]) expect(inCanvas(onScreen(container, uid)), uid).toBe(true);
    // Centred, as a fit is: the room left of the graph matches the room right of it.
    const left = onScreen(container, "start:flow").left;
    const right = 800 - onScreen(container, "end:flow").right;
    expect(Math.abs(left - right)).toBeLessThan(2);
  });

  it("a poll or an edit of the same flow keeps the user's view; another flow gets a first view", async () => {
    const user = userEvent.setup();
    const { container, rerender, props } = renderView({ graph: chain(30), viewKey: "flow-a" });
    await settle();
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await settle();
    const mine = viewportOf(container);
    expect(mine.zoom).toBeLessThan(READABLE_ZOOM);
    const draw = (over: Partial<TopologyViewProps>) =>
      rerender(
        <div style={{ width: 800, height: 600 }}>
          <TopologyView {...props} {...over} />
        </div>,
      );
    // A poll: the same graph, new states.
    draw({ graph: chain(30), viewKey: "flow-a", states: { "tasks.t0": STATES["tasks.a"] } });
    await settle();
    expect(viewportOf(container)).toEqual(mine);
    // An edit: another graph of the same flow.
    draw({ graph: chain(31), viewKey: "flow-a" });
    await settle();
    expect(viewportOf(container)).toEqual(mine);
    // Another flow.
    draw({ graph: chain(31), viewKey: "flow-b" });
    await settle();
    expect(viewportOf(container).zoom).toBeCloseTo(READABLE_ZOOM, 5);
  });
});

describe("TopologyView controls", () => {
  it("zooms in, zooms out and fits", async () => {
    const user = userEvent.setup();
    const { container } = renderView();
    await settle();
    await user.click(screen.getByRole("button", { name: "Fit view" }));
    await settle();
    // Fitted, a graph this wide is drawn smaller than life size.
    const fitted = viewportScale(container);
    expect(fitted).toBeLessThan(1);
    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    await settle();
    const zoomedIn = viewportScale(container);
    expect(zoomedIn).toBeGreaterThan(fitted);
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await settle();
    expect(viewportScale(container)).toBeLessThan(fitted);
    await user.click(screen.getByRole("button", { name: "Fit view" }));
    await settle();
    expect(viewportScale(container)).toBeCloseTo(fitted, 5);
  });

  it("switches orientation and remembers it under its key", async () => {
    const user = userEvent.setup();
    const { unmount } = renderView({ graph: LINE });
    expect(screen.getByRole("button", { name: "Horizontal" })).toHaveAttribute("aria-pressed", "true");
    const x = (uid: string) => Number(/translate\(([-\d.]+)px/.exec((node(uid).closest(".react-flow__node") as HTMLElement).style.transform)?.[1]);
    expect(x("tasks.two")).toBeGreaterThan(x("tasks.one"));
    await user.click(screen.getByRole("button", { name: "Vertical" }));
    expect(screen.getByRole("button", { name: "Vertical" })).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem(ORIENTATION_KEY)).toBe("TB");
    // Vertical: one under the other, so the same column.
    expect(x("tasks.two")).toBeCloseTo(x("tasks.one"), 0);
    unmount();
    renderView({ graph: LINE });
    expect(screen.getByRole("button", { name: "Vertical" })).toHaveAttribute("aria-pressed", "true");
  });

  it("ignores a stored orientation it does not know", () => {
    localStorage.setItem(ORIENTATION_KEY, "diagonal");
    renderView({ graph: LINE });
    expect(screen.getByRole("button", { name: "Horizontal" })).toHaveAttribute("aria-pressed", "true");
  });

  it("collapses a cluster, and Expand all opens it again", async () => {
    const user = userEvent.setup();
    renderView({ graph: NESTED });
    expect(hasNode("tasks.s.p.z")).toBe(true);
    await user.click(screen.getByRole("button", { name: "Collapse s" }));
    expect(hasNode("tasks.s.p.z")).toBe(false);
    expect(hasNode("tasks.s.p")).toBe(false);
    // The collapsed cluster is one node standing for the flowable.
    expect(node("cluster:tasks.s")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Expand s" })).toHaveAttribute("aria-expanded", "false");
    await user.click(screen.getByRole("button", { name: "Expand all" }));
    expect(hasNode("tasks.s.p.z")).toBe(true);
    expect(screen.getByRole("button", { name: "Collapse s" })).toHaveAttribute("aria-expanded", "true");
  });

  it("Collapse all folds every top-level cluster", async () => {
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(hasNode("tasks.p.x")).toBe(false);
    expect(hasNode("tasks.check.yes")).toBe(false);
    expect(hasNode("triggers.daily")).toBe(false);
    expect(hasNode("tasks.a")).toBe(true);
  });

  it("Expand on a Subflow asks for its expansion", async () => {
    const user = userEvent.setup();
    const { props } = renderView();
    await user.click(screen.getByRole("button", { name: "Expand subflow call2" }));
    expect(props.onExpandSubflow).toHaveBeenCalledWith("tasks.call2");
    expect(props.onSelect).not.toHaveBeenCalled();
  });

  it("selecting a collapsed cluster selects its flowable", async () => {
    const user = userEvent.setup();
    const { props } = renderView({ graph: NESTED });
    await user.click(screen.getByRole("button", { name: "Collapse s" }));
    await user.click(node("cluster:tasks.s"));
    expect(props.onSelect).toHaveBeenCalledWith("tasks.s");
  });
});

describe("TopologyView keyboard", () => {
  it("nodes are buttons with one tab stop; arrows walk the layout order; Enter selects", async () => {
    const user = userEvent.setup();
    const { props } = renderView({ graph: LINE });
    const stops = ["tasks.one", "tasks.two", "tasks.three"].map((uid) => node(uid).tabIndex);
    expect(stops).toEqual([0, -1, -1]);
    act(() => node("tasks.one").focus());
    expect(node("tasks.one")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(node("tasks.two")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(node("tasks.three")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(node("tasks.three")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(node("tasks.two")).toHaveFocus();
    expect(node("tasks.two").tabIndex).toBe(0);
    await user.keyboard("{Home}");
    expect(node("tasks.one")).toHaveFocus();
    await user.keyboard("{End}");
    expect(node("tasks.three")).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(props.onSelect).toHaveBeenCalledWith("tasks.three");
  });

  it("marks the selected node", () => {
    renderView({ graph: LINE, selectedUid: "tasks.two" });
    expect(node("tasks.two")).toHaveAttribute("aria-current", "true");
    expect(node("tasks.one")).not.toHaveAttribute("aria-current");
  });
});

describe("TopologyView list view", () => {
  it("replaces the canvas with a treegrid nested by cluster, carrying the same states", async () => {
    const user = userEvent.setup();
    const { props } = renderView({ states: STATES });
    await user.click(screen.getByRole("button", { name: "List view" }));
    expect(screen.getByRole("button", { name: "List view" })).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector(".react-flow")).toBeNull();
    const grid = screen.getByRole("treegrid", { name: "Tasks of the flow" });
    const rows = within(grid).getAllByRole("row").slice(1);
    const row = (uid: string) => {
      const r = rows.find((el) => el.getAttribute("data-uid") === uid);
      if (!r) throw new Error(`no row ${uid}`);
      return r;
    };
    // Every task node has a row, the nested ones included.
    for (const uid of ["tasks.a", "tasks.p", "tasks.p.x", "tasks.p.y", "tasks.check.yes", "errors.alert", "triggers.daily"]) expect(row(uid)).toBeTruthy();
    expect(within(row("tasks.p")).getByText("Failed")).toBeInTheDocument();
    expect(within(row("tasks.a")).getByText("Success")).toBeInTheDocument();
    expect(within(row("tasks.a")).getByText("3 s")).toBeInTheDocument();
    expect(within(row("tasks.p.y")).getByText("2 of 3 iterations done, 1 failed")).toBeInTheDocument();
    expect(within(row("tasks.p.y")).getByText("up to 3 attempts")).toBeInTheDocument();
    expect(within(row("tasks.check.yes")).getByText("Not run")).toBeInTheDocument();
    expect(within(row("tasks.bad")).getByText(/unsupported_task_type/)).toBeInTheDocument();
    await user.click(row("tasks.a"));
    expect(props.onSelect).toHaveBeenCalledWith("tasks.a");
  });

  it("uses the Gantt's treegrid ARIA: level, place among siblings, expansion, one tab stop", async () => {
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "List view" }));
    const grid = screen.getByRole("treegrid", { name: "Tasks of the flow" });
    const rows = within(grid).getAllByRole("row");
    expect(grid).toHaveAttribute("aria-rowcount", String(rows.length));
    const body = rows.slice(1);
    const aria = (uid: string) => {
      const r = body.find((el) => el.getAttribute("data-uid") === uid);
      if (!r) throw new Error(`no row ${uid}`);
      return [r.getAttribute("aria-level"), r.getAttribute("aria-posinset"), r.getAttribute("aria-setsize"), r.getAttribute("aria-expanded")];
    };
    // Triggers first, as Kestra draws them; then the flow's tasks in walk order.
    expect(body[0]).toHaveAttribute("data-uid", "cluster:triggers");
    expect(aria("cluster:triggers")).toEqual(["1", "1", "10", "true"]);
    expect(aria("triggers.daily")).toEqual(["2", "1", "1", null]);
    // After the triggers and `a` (dagre draws the `errors:` branch beside the end, by its successor).
    expect(aria("tasks.p")).toEqual(["1", "3", "10", "true"]);
    expect([aria("tasks.p.x"), aria("tasks.p.y")].map(([level, , size, expanded]) => [level, size, expanded])).toEqual([
      ["2", "2", null],
      ["2", "2", null],
    ]);
    expect([aria("tasks.p.x")[1], aria("tasks.p.y")[1]].sort()).toEqual(["1", "2"]);
    expect(body.filter((r) => r.tabIndex === 0)).toHaveLength(1);
    body[0].focus();
    await user.keyboard("{ArrowDown}");
    expect(body[1]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(body[0]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(aria("cluster:triggers")[3]).toBe("false");
    expect(within(grid).queryAllByRole("row").some((r) => r.getAttribute("data-uid") === "triggers.daily")).toBe(false);
    await user.keyboard("{ArrowRight}");
    expect(within(grid).queryAllByRole("row").some((r) => r.getAttribute("data-uid") === "triggers.daily")).toBe(true);
  });

  it("goes back to the graph", async () => {
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "List view" }));
    await user.click(screen.getByRole("button", { name: "List view" }));
    expect(screen.queryByRole("treegrid")).toBeNull();
    expect(hasNode("tasks.a")).toBe(true);
  });
});

describe("TopologyView with the largest graph the server returns", () => {
  // Role queries walk the accessibility tree of every one of 5,000 cards, which jsdom makes take
  // minutes; this test reaches the elements by selector and keeps role queries inside the toolbar.
  it("draws a chain of 5,000 tasks, and lists it", { timeout: 120_000 }, async () => {
    const user = userEvent.setup();
    const { container } = renderView({ graph: chain(5000) });
    const uids = () => [...container.querySelectorAll("[data-uid]")].map((el) => el.getAttribute("data-uid"));
    expect(container.querySelector("[role='alert']")).toBeNull();
    const drawn = uids();
    expect(drawn).toContain("tasks.t0");
    expect(drawn).toContain("tasks.t4999");
    const toolbar = container.querySelector("[role='toolbar']") as HTMLElement;
    await user.click(within(toolbar).getByRole("button", { name: "List view" }));
    const grid = container.querySelector("[role='treegrid']") as HTMLElement;
    expect(grid).toHaveAttribute("aria-label", "Tasks of the flow");
    const rows = [...grid.querySelectorAll("[role='row'][data-uid]")];
    expect(rows).toHaveLength(5000);
    expect(rows[0]).toHaveAttribute("data-uid", "tasks.t0");
    expect(rows[4999]).toHaveAttribute("data-uid", "tasks.t4999");
  });
});

describe("TopologyView states", () => {
  it("shows a skeleton while busy with nothing to draw yet", () => {
    const { container } = renderView({ graph: undefined, busy: true });
    expect(container.querySelector("[data-slot='skeleton']")).not.toBeNull();
    expect(container.querySelector(".react-flow")).toBeNull();
  });

  it("keeps the graph it has while a refresh is busy", () => {
    const { container } = renderView({ busy: true });
    expect(hasNode("tasks.a")).toBe(true);
    expect(container.querySelector("[data-slot='skeleton']")).toBeNull();
  });

  it("says when the flow has no tasks", () => {
    renderView({ graph: EMPTY_FLOW });
    expect(screen.getByText("This flow has no tasks")).toBeInTheDocument();
    expect(document.querySelector(".react-flow")).toBeNull();
  });

  it("keeps the last graph on an error and says why", () => {
    renderView({ error: "The server could not build the graph. Try again in a moment." });
    expect(screen.getByRole("alert")).toHaveTextContent("The server could not build the graph. Try again in a moment.");
    expect(hasNode("tasks.a")).toBe(true);
  });

  it("shows the error alone when there is no graph to keep, never the empty message", () => {
    renderView({ graph: undefined, error: "The graph could not be loaded." });
    expect(screen.getByRole("alert")).toHaveTextContent("The graph could not be loaded.");
    expect(screen.queryByText("This flow has no tasks")).toBeNull();
  });

  it("says the graph is out of date when stale", () => {
    renderView({ stale: true });
    expect(screen.getByText("The graph is out of date: the YAML has an error")).toBeInTheDocument();
    expect(hasNode("tasks.a")).toBe(true);
  });
});
