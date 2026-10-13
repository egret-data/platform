import { useId } from "react";
import type { RoutedEdge } from "@/components/workflows/topology/layout";
import { relationStroke } from "@/components/workflows/topology/topologyModel";

/**
 * THE EDGES, drawn along dagre's routes in the canvas's coordinates (inside
 * `ViewportPortal`, so they pan and zoom with the nodes).
 *
 * Not `@xyflow/react` edges: those attach to measured handles, so their path
 * would ignore the layout's route and, in jsdom (no layout), never render.
 * These are decoration for sighted users; the list view is the accessible
 * equivalent, so the layer is `aria-hidden`.
 *
 * Strokes are `var(--token)` in inline styles (FE-VISUAL-2): an SVG
 * presentation attribute does not resolve a CSS variable, a style does.
 * Trigger edges are dashed (spec §6.1); a branch's colour is repeated in words
 * on its nodes (FE-A11Y-4).
 */
/**
 * THE PAINT ORDER, in the canvas's one stacking context (xyflow's viewport): cluster boxes at
 * their nesting depth (0, 1, …), then this layer at `EDGE_Z`, then every card and marker at
 * `CARD_Z`. So a route crosses a cluster's tint but passes beneath a card. The labels share this
 * layer; dagre reserves each label a box of its own, so no card sits over one.
 */
export const EDGE_Z = 500;
export const CARD_Z = 1000;

const STROKES = ["var(--muted-foreground)", "var(--destructive)", "var(--highlight)"] as const;

function pathOf(points: RoutedEdge["points"]): string {
  return points.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");
}

export function TopologyEdges({ edges }: { edges: RoutedEdge[] }) {
  const prefix = useId().replace(/:/g, "");
  const markerId = (stroke: string) => `${prefix}-arrow-${STROKES.indexOf(stroke as (typeof STROKES)[number])}`;
  const labelled = edges.filter((e) => e.labelAt && e.label);
  return (
    <>
      <svg aria-hidden data-edge-layer className="pointer-events-none absolute top-0 left-0 overflow-visible" style={{ zIndex: EDGE_Z }} width={1} height={1}>
        <defs>
          {STROKES.map((stroke) => (
            <marker key={stroke} id={markerId(stroke)} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
              <path d="M 0 0 L 10 5 L 0 10 z" style={{ fill: stroke }} />
            </marker>
          ))}
        </defs>
        {edges.map((e) => {
          const stroke = relationStroke(e.relation);
          return (
            <path
              key={e.id}
              data-edge={`${e.source} -> ${e.target}`}
              data-relation={e.relation}
              d={pathOf(e.points)}
              markerEnd={`url(#${markerId(stroke)})`}
              style={{
                stroke,
                strokeWidth: 1.5,
                fill: "none",
                strokeLinejoin: "round",
                strokeDasharray: e.trigger ? "6 4" : undefined,
              }}
            />
          );
        })}
      </svg>
      <div data-edge-labels aria-hidden className="pointer-events-none absolute top-0 left-0" style={{ zIndex: EDGE_Z }}>
        {labelled.map((e) => (
          <span
            key={e.id}
            className="absolute -translate-x-1/2 -translate-y-1/2 rounded-sm border border-border bg-background px-1 font-mono text-xs whitespace-nowrap text-foreground"
            style={{ left: e.labelAt?.x, top: e.labelAt?.y }}
          >
            {e.label}
          </span>
        ))}
      </div>
    </>
  );
}
