import "@xyflow/react/dist/base.css";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ReactFlow, ReactFlowProvider, ViewportPortal, useReactFlow, useStore } from "@xyflow/react";
import { List, Maximize, ZoomIn, ZoomOut } from "lucide-react";
import type { NamespaceDependenciesDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { DependenciesList } from "@/components/workflows/namespaces/dependencies/DependenciesList";
import { layoutDependencies, type FlowBox } from "@/components/workflows/namespaces/dependencies/dependenciesModel";
import { DependenciesContext, NODE_TYPES, type DependenciesContextValue, type FlowCardNode } from "@/components/workflows/namespaces/dependencies/DependencyNode";
import { FIT, MAX_ZOOM, MIN_ZOOM, XY_THEME, rovingTarget, useKeepInView } from "@/components/workflows/topology/canvas";
import { CARD_Z, TopologyEdges } from "@/components/workflows/topology/TopologyEdges";
import { LAYOUT_FAILED, firstView } from "@/components/workflows/topology/TopologyView";
import { cn } from "@/lib/utils";

/**
 * THE NAMESPACE DEPENDENCIES GRAPH (W7d spec §4.2), drawn with W7c's topology pieces: the layout's
 * `arrange` (dagre, or its layered fallback), the edge layer (`TopologyEdges`: `subflow` solid,
 * `flow_trigger` dashed), the canvas's theme, zoom range and roving keys (`topology/canvas.ts`), and
 * the first view (`firstView`: the fit when readable, else the readable zoom on the first flow).
 *
 * Read-only: nothing is dragged, connected or deleted. A card is a link to its flow; the cards
 * share one tab stop, the arrows walk the layout's order. The list view is the accessible
 * equivalent. A graph the browser cannot lay out shows W7c's alert and offers the list view.
 *
 * This module brings `@xyflow/react` and dagre; the Dependencies tab reaches it, and is reached
 * from the namespace detail through `lazy()` only (`scripts/assert-workflows-lazy.mjs`).
 */

/**
 * Past this many flows, xyflow draws only the cards on screen. The server allows 2,000 flows; a
 * focus moved to a card not drawn yet lands when it is drawn (`pending`).
 */
const VISIBLE_ONLY_FROM = 500;

/** The legend's sentences, the edge kinds in words (FE-A11Y-4: the dash is not the only cue). */
export const LEGEND = {
  subflow: "Subflow: the flow calls the next",
  flow_trigger: "Flow trigger: the flow's end starts the next",
} as const;

export interface DependenciesViewProps {
  deps: NamespaceDependenciesDto;
  flowHref: (id: string) => string;
}

export function DependenciesView(props: DependenciesViewProps) {
  return (
    <ReactFlowProvider>
      <Dependencies {...props} />
    </ReactFlowProvider>
  );
}

const FAILED = Symbol("layout failed");

function Dependencies({ deps, flowHref }: DependenciesViewProps) {
  const [listView, setListView] = useState(false);
  const [focusId, setFocusId] = useState<string | null>(null);
  const links = useRef(new Map<string, HTMLAnchorElement>());
  const pending = useRef<string | null>(null);
  const rf = useReactFlow();
  const keepInView = useKeepInView();

  const drawn = useMemo(() => {
    try {
      return layoutDependencies(deps);
    } catch {
      return FAILED;
    }
  }, [deps]);
  const placed = drawn === FAILED ? null : drawn;
  const nodes = useMemo(() => placed?.nodes ?? [], [placed]);
  const tabStop = nodes.some((n) => n.id === focusId) ? focusId : (nodes[0]?.id ?? null);

  const moveTo = useCallback(
    (box: FlowBox) => {
      setFocusId(box.id);
      const el = links.current.get(box.id);
      pending.current = el ? null : box.id;
      el?.focus();
      keepInView(box);
    },
    [keepInView],
  );

  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLAnchorElement>, id: string) => {
      const target = rovingTarget(e.key, nodes, nodes.findIndex((n) => n.id === id));
      // Enter reaches the link itself.
      if (target === null) return;
      e.preventDefault();
      if (target) moveTo(target);
    },
    [nodes, moveTo],
  );

  const ctx: DependenciesContextValue = {
    flowHref,
    tabStop,
    onKeyDown,
    onFocus: setFocusId,
    register: (id, el) => {
      if (!el) {
        links.current.delete(id);
        return;
      }
      links.current.set(id, el);
      if (pending.current === id) {
        pending.current = null;
        el.focus();
      }
    },
  };

  const flowNodes = useMemo<FlowCardNode[]>(
    () =>
      nodes.map((box) => ({
        id: box.id,
        type: "flow",
        position: { x: box.x, y: box.y },
        width: box.width,
        height: box.height,
        data: { box },
        draggable: false,
        connectable: false,
        selectable: false,
        focusable: false,
        deletable: false,
        zIndex: CARD_Z,
      })),
    [nodes],
  );

  // The first view: placed once the canvas has a size, and again for another graph.
  const canvasWidth = useStore((s) => s.width);
  const canvasHeight = useStore((s) => s.height);
  const viewed = useRef<unknown>(null);
  const canvasShown = !listView && placed !== null;
  useEffect(() => {
    if (!canvasShown) viewed.current = null;
  }, [canvasShown]);
  useEffect(() => {
    if (!placed || !canvasShown || canvasWidth <= 0 || canvasHeight <= 0 || viewed.current === placed) return;
    viewed.current = placed;
    void rf.setViewport(firstView(placed.nodes, "LR", canvasWidth, canvasHeight));
  }, [placed, canvasShown, canvasWidth, canvasHeight, rf]);

  let body;
  if (listView) {
    body = <DependenciesList deps={deps} flowHref={flowHref} />;
  } else if (!placed) {
    body = (
      <div role="alert" className="flex flex-col items-start gap-2 rounded-md border border-destructive bg-destructive/15 px-3 py-3 text-sm text-foreground">
        <p className="font-medium">{LAYOUT_FAILED}</p>
        <p>Use the list view to read its flows and their dependencies.</p>
        <Button variant="outline" size="sm" onClick={() => setListView(true)}>
          <List aria-hidden className="size-4" />
          Show the list view
        </Button>
      </div>
    );
  } else {
    body = (
      <div className="h-full min-h-64 w-full rounded-md border bg-background" style={XY_THEME}>
        <DependenciesContext.Provider value={ctx}>
          <ReactFlow
            nodes={flowNodes}
            edges={[]}
            nodeTypes={NODE_TYPES}
            minZoom={MIN_ZOOM}
            maxZoom={MAX_ZOOM}
            onlyRenderVisibleElements={nodes.length >= VISIBLE_ONLY_FROM}
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
            aria-label="Dependencies of the namespace"
          >
            <ViewportPortal>
              <TopologyEdges edges={placed.edges} />
            </ViewportPortal>
          </ReactFlow>
        </DependenciesContext.Provider>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <div role="toolbar" aria-label="Dependencies" className="flex flex-wrap items-center gap-1">
          {canvasShown ? (
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
            </>
          ) : null}
          <Button variant="ghost" size="sm" aria-pressed={listView} className={cn("h-8", listView && "bg-muted")} onClick={() => setListView((v) => !v)}>
            <List aria-hidden className="size-4" />
            List view
          </Button>
        </div>
        <ul role="list" aria-label="Edges" className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <li className="flex items-center gap-1.5">
            <EdgeSample dashed={false} />
            {LEGEND.subflow}
          </li>
          <li className="flex items-center gap-1.5">
            <EdgeSample dashed />
            {LEGEND.flow_trigger}
          </li>
        </ul>
      </div>
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}

/** A short stretch of an edge as the canvas draws it (`TopologyEdges`). */
function EdgeSample({ dashed }: { dashed: boolean }) {
  return (
    <svg aria-hidden width={24} height={8} className="shrink-0">
      <line x1={0} y1={4} x2={24} y2={4} style={{ stroke: "var(--muted-foreground)", strokeWidth: 1.5, strokeDasharray: dashed ? "6 4" : undefined }} />
    </svg>
  );
}
