import "@xyflow/react/dist/base.css";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { ReactFlow, ReactFlowProvider, ViewportPortal, getViewportForBounds, useReactFlow, useStore, useStoreApi, type Node as FlowNode, type Viewport } from "@xyflow/react";
import { ChevronsDownUp, ChevronsUpDown, GitFork, List, Maximize, ZoomIn, ZoomOut } from "lucide-react";
import type { FlowGraph, NodeState } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { isTerminalState } from "@/components/workflows/executionState";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Skeleton } from "@/components/ui/skeleton";
import { layout, type LayoutOptions, type Orientation, type Positioned } from "@/components/workflows/topology/layout";
import { NODE_TYPES, TopologyContext, type TopologyContextValue, type TopologyFlowNode } from "@/components/workflows/topology/TopologyNodes";
import { CARD_Z, TopologyEdges } from "@/components/workflows/topology/TopologyEdges";
import { TopologyListView } from "@/components/workflows/topology/TopologyListView";
import { cn } from "@/lib/utils";

/**
 * THE TOPOLOGY OF A FLOW (W7c spec §6.1), shared by the editor's Topology tab
 * (the unsaved YAML's graph) and the execution's (with `states`). Read-only:
 * nodes are not draggable, connectable or deletable, and selecting one only
 * tells the tab (`onSelect`).
 *
 * The server builds the graph; `layout()` places it with dagre; `@xyflow/react`
 * pans, zooms and fits. This module and its imports (xyflow, its stylesheet,
 * dagre) form the Topology chunk: the tabs reach it through `lazy()` only, and
 * `scripts/assert-workflows-lazy.mjs` checks that none of it reaches the entry
 * chunk or the flow list's (`ORIENTATION_KEY` is this module's marker there).
 */

/** Where the orientation is remembered (a per-viewer preference, FE-DATA-3). Also this module's bundle marker. */
export const ORIENTATION_KEY = "egret.workflows.topology.orientation";

/** What the canvas says when the graph cannot be laid out (FE-STATE-1: an error, never a blank canvas). */
export const LAYOUT_FAILED = "This graph could not be drawn.";

const FAILED = Symbol("layout failed");

/**
 * `layout()`, never throwing. dagre's limits are handled inside it (`DAGRE_LIMIT`); this is the
 * net for anything else, so a graph the browser cannot lay out costs the canvas, not the tab.
 */
function tryLayout(graph: FlowGraph, opts: LayoutOptions): ReturnType<typeof layout> | typeof FAILED {
  try {
    return layout(graph, opts);
  } catch {
    return FAILED;
  }
}

/** §6.2's banner while the editor's YAML does not parse. */
export const STALE_BANNER = "The graph is out of date: the YAML has an error";

function readOrientation(): Orientation {
  try {
    const v = localStorage.getItem(ORIENTATION_KEY);
    return v === "TB" ? "TB" : "LR";
  } catch {
    return "LR";
  }
}

function writeOrientation(o: Orientation): void {
  try {
    localStorage.setItem(ORIENTATION_KEY, o);
  } catch {
    // A per-viewer preference: without storage it lasts for this visit.
  }
}

/** The clock running durations are measured to; it ticks once a second only while a node runs. */
function useClock(ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [ticking]);
  return now;
}

export interface TopologyViewProps {
  /** The graph to draw; keep passing the last good one while a refresh fails (`error`) or the YAML is broken (`stale`). */
  graph: FlowGraph | null | undefined;
  /** An execution's node states; without them nothing is dimmed for want of a run. */
  states?: Readonly<Record<string, NodeState>>;
  selectedUid?: string | null;
  onSelect: (uid: string) => void;
  /** A `Subflow` node's Expand: the tab refetches with this uid in `subflows`. */
  onExpandSubflow: (uid: string) => void;
  /** A request is in flight. With no graph yet, a skeleton is drawn. */
  busy?: boolean;
  /** Why the last request failed, in words for the user (FE-COPY-2). */
  error?: string | null;
  /** The graph no longer matches the source (the editor's YAML does not parse). */
  stale?: boolean;
  /**
   * What the graph is OF (a flow, an execution). The first view is placed when the first graph
   * arrives and again when this changes, not when a poll or an edit brings a new graph of the
   * same thing: the user's pan and zoom stay.
   */
  viewKey?: string;
}

/**
 * xyflow's stylesheet is unlayered, so it beats Tailwind's utilities; its own variables are the way
 * in. Its attribution's background defaults to a light wash, which shows as a pale box on dark.
 */
const XY_THEME = { "--xy-attribution-background-color": "var(--background)" } as CSSProperties;

const FOCUSABLE = new Set<Positioned["kind"]>(["task", "trigger", "collapsed"]);
/** Fit: room around the graph, and never blown up past life size. */
const FIT = { padding: 0.12, maxZoom: 1 } as const;
/** The canvas's zoom range; Fit may go as low as `MIN_ZOOM` on a large graph. */
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 2;

/**
 * The FIRST view's least zoom. A card's task id is `text-sm` in the mono font, 13 px as the
 * theme sets it (measured in the browser), so at 0.8 it is drawn at 10.4 px. A fit that would go lower (a 15-task flow fits at about 0.2 in a 600 px panel) is not
 * a view anyone can read; the first view stays at this zoom instead and shows where the flow
 * starts. The Fit button still fits the whole graph, at any zoom down to `MIN_ZOOM`.
 */
export const READABLE_ZOOM = 0.8;
/** Room between the canvas's leading edge and the graph in the first view, in screen pixels. */
const LEAD = 24;

/** How much of the edge into the flow's start the first view keeps before it, in graph pixels. */
const BEFORE_START = 16;

/**
 * Where the first view of a graph looks: the fit, when it is readable; else `READABLE_ZOOM`,
 * anchored on the flow's start (`start:flow`) and its first task, the first card of the flow's
 * `tasks:` list along the layout. Along the main axis (x when horizontal, y when vertical) the
 * view starts at the graph's leading edge when the first task still fits in the canvas from there,
 * which keeps a trigger chain on screen; otherwise just before `start:flow`, so the triggers are
 * off-screen and the start and the first task are not. Across, it centres the two.
 */
export function firstView(nodes: readonly Positioned[], orientation: Orientation, width: number, height: number): Viewport {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of nodes) {
    minX = Math.min(minX, n.x);
    minY = Math.min(minY, n.y);
    maxX = Math.max(maxX, n.x + n.width);
    maxY = Math.max(maxY, n.y + n.height);
  }
  if (minX === Infinity) return { x: 0, y: 0, zoom: 1 };
  const fit = getViewportForBounds({ x: minX, y: minY, width: maxX - minX, height: maxY - minY }, width, height, MIN_ZOOM, FIT.maxZoom, FIT.padding);
  if (fit.zoom >= READABLE_ZOOM) return fit;
  const z = READABLE_ZOOM;
  const lr = orientation === "LR";
  // Main axis position and length, and cross axis, of a box.
  const main = (n: Positioned) => (lr ? n.x : n.y);
  const mainLen = (n: Positioned) => (lr ? n.width : n.height);
  const cross = (n: Positioned) => (lr ? n.y : n.x);
  const crossLen = (n: Positioned) => (lr ? n.height : n.width);
  const start = nodes.find((n) => n.id === "start:flow") ?? nodes[0];
  let first: Positioned | undefined;
  for (const n of nodes) {
    if (n.node?.node !== "task" || n.node.branch !== "tasks" || main(n) < main(start)) continue;
    if (!first || main(n) < main(first)) first = n;
  }
  first ??= start;
  const span = lr ? width : height;
  const graphLead = lr ? minX : minY;
  // From the graph's leading edge, the first task's far edge lands here on screen.
  const fromGraphLead = LEAD + (main(first) + mainLen(first) - graphLead) * z;
  const anchor = fromGraphLead <= span - LEAD ? graphLead : main(start) - BEFORE_START;
  const lo = Math.min(cross(start), cross(first));
  const hi = Math.max(cross(start) + crossLen(start), cross(first) + crossLen(first));
  const alongMain = LEAD - anchor * z;
  const across = (lr ? height : width) / 2 - ((lo + hi) / 2) * z;
  return lr ? { zoom: z, x: alongMain, y: across } : { zoom: z, x: across, y: alongMain };
}

export function TopologyView(props: TopologyViewProps) {
  return (
    <ReactFlowProvider>
      <Topology {...props} />
    </ReactFlowProvider>
  );
}

function Topology({ graph, states, selectedUid = null, onSelect, onExpandSubflow, busy, error, stale, viewKey = "" }: TopologyViewProps) {
  const [orientation, setOrientation] = useState<Orientation>(readOrientation);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [listView, setListView] = useState(false);
  const [focusId, setFocusId] = useState<string | null>(null);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const rf = useReactFlow();
  const store = useStoreApi();
  const now = useClock(!!states && Object.values(states).some((s) => !!s.started_at && !s.ended_at && !isTerminalState(s.state)));

  const drawn = useMemo(() => (graph ? tryLayout(graph, { orientation, collapsed, states }) : null), [graph, orientation, collapsed, states]);
  const placed = drawn && drawn !== FAILED ? drawn : null;
  const layoutFailed = drawn === FAILED;
  // The list's order is the whole graph's, whatever is collapsed on the canvas. Without a layout
  // the list keeps the graph's own order: it never needs one.
  const fullOrder = useMemo(() => {
    if (!graph || !listView) return [];
    const whole = tryLayout(graph, { orientation, collapsed: new Set(), states });
    return whole === FAILED ? [] : whole.nodes;
  }, [graph, listView, orientation, states]);

  const focusable = useMemo(() => (placed ? placed.nodes.filter((n) => FOCUSABLE.has(n.kind)) : []), [placed]);
  const tabStop = focusable.some((n) => n.id === focusId) ? focusId : (focusable[0]?.id ?? null);

  const toggleCluster = useCallback((uid: string, open?: boolean) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      const shut = open === undefined ? !prev.has(uid) : !open;
      if (shut) next.add(uid);
      else next.delete(uid);
      return next;
    });
  }, []);

  const moveTo = useCallback(
    (item: Positioned) => {
      setFocusId(item.id);
      buttons.current.get(item.id)?.focus();
      // Keep the focused node on screen: centre it when it is outside the visible canvas.
      const { width, height } = store.getState();
      const { x, y, zoom } = rf.getViewport();
      const left = item.x * zoom + x;
      const top = item.y * zoom + y;
      if (width > 0 && (left < 0 || top < 0 || left + item.width * zoom > width || top + item.height * zoom > height)) {
        void rf.setCenter(item.x + item.width / 2, item.y + item.height / 2, { zoom });
      }
    },
    [rf, store],
  );

  const onNodeKeyDown = useCallback(
    (e: KeyboardEvent<HTMLButtonElement>, id: string) => {
      const i = focusable.findIndex((n) => n.id === id);
      let target: Positioned | undefined;
      switch (e.key) {
        case "ArrowRight":
        case "ArrowDown":
          target = focusable[i + 1];
          break;
        case "ArrowLeft":
        case "ArrowUp":
          target = focusable[i - 1];
          break;
        case "Home":
          target = focusable[0];
          break;
        case "End":
          target = focusable[focusable.length - 1];
          break;
        default:
          // Enter and Space reach the button's own click.
          return;
      }
      e.preventDefault();
      if (target) moveTo(target);
    },
    [focusable, moveTo],
  );

  const ctx: TopologyContextValue = {
    states,
    clusters: graph?.clusters ?? [],
    selectedUid,
    tabStop,
    now,
    onSelect,
    onExpandSubflow,
    toggleCluster,
    onNodeKeyDown,
    onNodeFocus: setFocusId,
    register: (id, el) => {
      if (el) buttons.current.set(id, el);
      else buttons.current.delete(id);
    },
  };

  const flowNodes = useMemo<TopologyFlowNode[]>(() => {
    if (!placed) return [];
    const at = new Map(placed.nodes.map((n) => [n.id, n]));
    const depthOf = (item: Positioned) => {
      let d = 0;
      for (let p = item.parent; p !== null; p = at.get(p)?.parent ?? null) d++;
      return d;
    };
    return placed.nodes.map((item): TopologyFlowNode => {
      const parent = item.parent ? at.get(item.parent) : undefined;
      const type = item.kind === "start" || item.kind === "end" ? "marker" : item.kind === "trigger" ? "task" : item.kind;
      const node: FlowNode<{ item: Positioned }> = {
        id: item.id,
        type,
        position: { x: item.x - (parent?.x ?? 0), y: item.y - (parent?.y ?? 0) },
        width: item.width,
        height: item.height,
        data: { item },
        draggable: false,
        connectable: false,
        selectable: false,
        focusable: false,
        deletable: false,
        // Boxes by depth beneath the edge layer, cards above it (`TopologyEdges.tsx`, paint order).
        zIndex: item.kind === "cluster" ? depthOf(item) : CARD_Z,
      };
      if (parent) node.parentId = parent.id;
      return node;
    });
  }, [placed]);

  // The first view (`firstView`): placed once the canvas has a size, and again for another flow or
  // execution (`viewKey`), another orientation or another set of collapsed clusters, which are the
  // user's own requests to redraw. A poll's states or an edit's graph of the same flow keep the view.
  const canvasWidth = useStore((s) => s.width);
  const canvasHeight = useStore((s) => s.height);
  const view = placed ? JSON.stringify([viewKey, orientation, [...collapsed].sort()]) : null;
  const viewed = useRef<string | null>(null);
  // Without the canvas (the list view, a failed layout) there is no view to keep: showing it again places the first view.
  const canvasShown = !listView && !layoutFailed;
  useEffect(() => {
    if (!canvasShown) viewed.current = null;
  }, [canvasShown]);
  useEffect(() => {
    if (!placed || view === null || canvasWidth <= 0 || canvasHeight <= 0 || viewed.current === view) return;
    viewed.current = view;
    void rf.setViewport(firstView(placed.nodes, orientation, canvasWidth, canvasHeight));
  }, [placed, view, orientation, canvasWidth, canvasHeight, rf]);

  const clusters = graph?.clusters ?? [];
  const collapseAll = () => setCollapsed(new Set(clusters.filter((c) => !c.reason && !c.parent).map((c) => c.uid)));
  const hasTasks = !!graph && graph.nodes.some((n) => n.node === "task");

  let body;
  if (!graph) {
    // FE-STATE-1: an error with nothing to keep is the error alone; loading is a skeleton (FE-STATE-2).
    body = error ? null : busy ? <Skeleton className="h-full min-h-48 w-full" /> : null;
  } else if (!hasTasks) {
    body = <EmptyState icon={GitFork} title="This flow has no tasks" description="Add a task to the YAML to see it here." />;
  } else if (listView) {
    body = (
      <TopologyListView
        graph={graph}
        order={fullOrder}
        states={states}
        collapsed={collapsed}
        onToggle={toggleCluster}
        selectedUid={selectedUid}
        onSelect={onSelect}
        now={now}
      />
    );
  } else if (layoutFailed) {
    body = (
      <div role="alert" className="flex flex-col items-start gap-2 rounded-md border border-destructive bg-destructive/15 px-3 py-3 text-sm text-foreground">
        <p className="font-medium">{LAYOUT_FAILED}</p>
        {/* The editor's graph has no states (nothing has run); the execution's does. */}
        <p>{states ? "Use the list view to read its tasks and their states." : "Use the list view to read its tasks."}</p>
        <Button variant="outline" size="sm" onClick={() => setListView(true)}>
          <List aria-hidden className="size-4" />
          Show the list view
        </Button>
      </div>
    );
  } else {
    body = (
      <div className="h-full min-h-64 w-full rounded-md border bg-background" style={XY_THEME}>
        <TopologyContext.Provider value={ctx}>
          <ReactFlow
            nodes={flowNodes}
            edges={[]}
            nodeTypes={NODE_TYPES}
            minZoom={MIN_ZOOM}
            maxZoom={MAX_ZOOM}
            nodesDraggable={false}
            nodesConnectable={false}
            nodesFocusable={false}
            edgesFocusable={false}
            elementsSelectable={false}
            disableKeyboardA11y
            deleteKeyCode={null}
            selectionKeyCode={null}
            multiSelectionKeyCode={null}
            zoomOnDoubleClick={false}
            aria-label="Topology of the flow"
          >
            <ViewportPortal>
              <TopologyEdges edges={placed?.edges ?? []} />
            </ViewportPortal>
          </ReactFlow>
        </TopologyContext.Provider>
      </div>
    );
  }

  // Zoom, fit and orientation act on a drawing; without one they are not offered.
  const canvasControls = !!graph && hasTasks && !listView && !layoutFailed;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      {graph && hasTasks ? (
        <div role="toolbar" aria-label="Topology" className="flex flex-wrap items-center gap-1">
          {canvasControls ? (
            <>
              <IconButton aria-label="Zoom in" title="Zoom in" onClick={() => void rf.zoomIn()} className="size-8">
                <ZoomIn aria-hidden className="size-4" />
              </IconButton>
              <IconButton aria-label="Zoom out" title="Zoom out" onClick={() => void rf.zoomOut()} className="size-8">
                <ZoomOut aria-hidden className="size-4" />
              </IconButton>
              <IconButton aria-label="Fit view" title="Fit view" onClick={() => void rf.fitView(FIT)} className="size-8">
                <Maximize aria-hidden className="size-4" />
              </IconButton>
              <div role="group" aria-label="Orientation" className="ml-1 inline-flex rounded-md border">
                {(["LR", "TB"] as const).map((o) => (
                  <Button
                    key={o}
                    variant="ghost"
                    size="sm"
                    aria-pressed={orientation === o}
                    onClick={() => {
                      setOrientation(o);
                      writeOrientation(o);
                    }}
                    className={cn("h-8 rounded-none first:rounded-l-md last:rounded-r-md", orientation === o && "bg-muted")}
                  >
                    {o === "LR" ? "Horizontal" : "Vertical"}
                  </Button>
                ))}
              </div>
            </>
          ) : null}
          <Button variant="ghost" size="sm" className="h-8" onClick={() => setCollapsed(new Set())}>
            <ChevronsUpDown aria-hidden className="size-4" />
            Expand all
          </Button>
          <Button variant="ghost" size="sm" className="h-8" onClick={collapseAll}>
            <ChevronsDownUp aria-hidden className="size-4" />
            Collapse all
          </Button>
          <Button variant="ghost" size="sm" aria-pressed={listView} className={cn("ml-auto h-8", listView && "bg-muted")} onClick={() => setListView((v) => !v)}>
            <List aria-hidden className="size-4" />
            List view
          </Button>
        </div>
      ) : null}
      {stale ? (
        <p role="status" className="rounded-md border border-highlight bg-highlight/15 px-3 py-2 text-sm text-foreground">
          {STALE_BANNER}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="rounded-md border border-destructive bg-destructive/15 px-3 py-2 text-sm text-foreground">
          {error}
        </p>
      ) : null}
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}

