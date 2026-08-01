import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseOption, tooltipSurface } from "../base";
import { CHART_THEME, COLOR_FALLBACK, resolveColor, seriesColor } from "../palette";

export interface ResultColumn { name: string; type: string }

/** Metabase's `sankey.node_align`. */
export type NodeAlign = "left" | "right" | "justify";
/** Metabase's `sankey.edge_color`. */
export type EdgeColor = "gray" | "source" | "target";

export interface SankeyBuildConfig {
  source?: string;
  target?: string;
  metric?: string;
  /** Unset is "left", as Metabase defaults it. */
  nodeAlign?: NodeAlign;
  /** Unset is "source" — Metabase's default, and the one that lets you follow a flow by colour. */
  edgeColor?: EdgeColor;
  showEdgeLabels?: boolean;
  /** Metabase's `sankey.label_value_formatting`, shown only while edge labels are on. */
  labelFormatting?: "auto" | "compact" | "full";
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

/** Metabase's `MAX_SANKEY_NODES`. Past this the diagram is a grey smear and the browser struggles. */
export const MAX_SANKEY_NODES = 150;
/** `SANKEY_CHART_STYLE.edgeColor.gray` — its own neutral, not a palette accent. */
const GRAY_EDGE = "#81898e";
const NODE_WIDTH = 20;          // SANKEY_CHART_STYLE.nodeWidth
const LABEL_HALO = 3;           // nodeLabels.textBorderWidth
const EDGE_OPACITY = 0.2;
const CURVENESS = 0.5;

/**
 * Why the flow cannot be drawn, in words a reader can act on — Metabase's `checkRenderable`.
 *
 * egret had none of this, and the failure was ugly: a circular flow reached ECharts, which threw
 * **"Sankey is a DAG, the original data has cycle!"** straight onto the card in red. That is the
 * library's own internal wording, it names an acronym rather than a fix, and it is what a user saw
 * for the ordinary mistake of picking two columns that point at each other.
 */
export function sankeyProblem(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: SankeyBuildConfig,
): string | undefined {
  if (cfg.source == null || cfg.target == null || cfg.metric == null) return undefined;
  if (cfg.source === cfg.target) {
    return "Select two different columns for source and target to create a flow.";
  }
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const si = idx[cfg.source];
  const ti = idx[cfg.target];
  if (si == null || ti == null || rows.length === 0) return undefined;

  const nodes = new Set<string>();
  for (const r of rows) {
    nodes.add(String(r[si] ?? ""));
    nodes.add(String(r[ti] ?? ""));
  }
  if (nodes.size > MAX_SANKEY_NODES) {
    return `A sankey chart can't show more than ${MAX_SANKEY_NODES} unique nodes — this one has ${nodes.size}.`;
  }
  if (hasCyclicFlow(rows, si, ti)) {
    return "Selected columns create circular flows. Try picking different columns that flow in one direction.";
  }
  return undefined;
}

/**
 * Depth-first cycle detection over the source→target graph — Metabase's `hasCyclicFlow`.
 *
 * `path` is the nodes on the CURRENT descent, not everything seen: a node reachable by two separate
 * routes is a diamond, which a sankey draws happily, and only a node reached again while still on
 * the stack is a loop. Iterative rather than recursive so a long chain cannot blow the stack on the
 * render path.
 */
export function hasCyclicFlow(rows: unknown[][], sourceIndex: number, targetIndex: number): boolean {
  const graph = new Map<string, Set<string>>();
  for (const r of rows) {
    const s = String(r[sourceIndex] ?? "");
    const t = String(r[targetIndex] ?? "");
    const edges = graph.get(s);
    if (edges) edges.add(t); else graph.set(s, new Set([t]));
  }

  const visited = new Set<string>();
  const path = new Set<string>();
  for (const root of graph.keys()) {
    if (visited.has(root)) continue;
    // Each frame is [node, its still-unvisited neighbours]; popping a frame leaves that node's
    // subtree, which is when it stops being "on the current path".
    const stack: [string, string[]][] = [[root, [...(graph.get(root) ?? [])]]];
    visited.add(root);
    path.add(root);
    while (stack.length > 0) {
      const frame = stack[stack.length - 1];
      const next = frame[1].pop();
      if (next === undefined) {
        path.delete(frame[0]);
        stack.pop();
        continue;
      }
      if (path.has(next)) return true;
      if (visited.has(next)) continue;
      visited.add(next);
      path.add(next);
      stack.push([next, [...(graph.get(next) ?? [])]]);
    }
  }
  return false;
}

// Metabase-style Sankey: weighted flows from source category to target category.
export function buildSankeyOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: SankeyBuildConfig,
  /** The chart's measured pixel size, when the caller has one — this builder is pure data with no
   * DOM of its own, so EChartsRenderer measures the container and threads it through (see
   * `buildPieOption`, the first to take one).
   *
   * Unread today. It is the seam node-label truncation attaches to: how much room a node's label
   * has before it runs into the next column is a question about WIDTH divided between the depths,
   * and this builder has never been able to see it. Today it asks for full labels and loses them
   * two ways instead. `labelLayout.hideOverlap` below drops outright any label that would collide
   * with another — so a lost label is GONE, not overlapping. And ECharts' `label.position` defaults
   * to "right", which this file does not override, so every label is drawn to the right of its
   * node, including the last column's; those have only the `right: "8%"` gutter set below to live
   * in — ECharts' own default is 20% — and no `label.width` or `label.overflow` is set anywhere, so
   * ECharts does no ellipsis of its own and the container edge simply cuts the text. That is why a
   * narrow card shows one- and two-character node names on the target side
   * (docs/en/specs/2026-08-01-chart-parity-design.md).
   *
   * Undefined means the caller has no measurement (every unit test, and any caller passing a plain
   * option object rather than a size function), and labels keep exactly that behaviour rather than
   * being fitted against a guessed width.
   *
   * `__tests__/sizeAware.test.ts` pins that passing a size changes nothing yet — if you are about
   * to delete this parameter as unused, that test is the thing to read first. */
  _size?: EChartsSize,
): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const si = cfg.source != null ? idx[cfg.source] : undefined;
  const ti = cfg.target != null ? idx[cfg.target] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const links: { source: string; target: string; value: number }[] = [];
  const names = new Set<string>();
  if (si != null && ti != null && mi != null) {
    for (const r of rows) {
      const value = Number(r[mi]);
      if (!Number.isFinite(value) || value <= 0) continue;
      const source = String(r[si] ?? "");
      const target = String(r[ti] ?? "");
      links.push({ source, target, value });
      names.add(source);
      names.add(target);
    }
  }
  // Each node states its OWN colour, cycling egret's accents. Left to itself ECharts INTERPOLATES
  // along the palette array for a sankey rather than picking discrete entries: measured on seven
  // nodes it drew #7EB967, #F3AF6C, #F6BF6A, #CA8AAA, #61A8B5 — five colours that are in no design
  // system, muddied versions sitting between two accents. Metabase's nodes measure as clean
  // accents (#88BF4D, #509EE3, #EF8C8C, #F9D45C, #98D9D9), which is what naming them here gives.
  const data = [...names].map((name, i) => ({ name, itemStyle: { color: seriesColor(i) } }));

  const widest = links.reduce((n, l) => Math.max(n, l.value), 0);
  const fmt = (v: unknown, compact?: boolean) => {
    if (cfg.metric == null) return String(v);
    const col = cfg.columnSettings?.[cfg.metric];
    return formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", {
      ...col,
      ...(compact != null ? { compact: col?.compact ?? compact } : {}),
    });
  };
  // "Auto" shortens once the numbers get wide, the same 10,000 threshold every other chart uses.
  const labelCompact = cfg.labelFormatting === "compact" ? true
    : cfg.labelFormatting === "full" ? false
      : widest >= 10_000;

  const ink = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const halo = resolveColor("var(--background)", "#ffffff");
  // Metabase colours an edge by one of its ENDS rather than blending between them: a gradient
  // belongs to neither node, so it cannot be followed. "source" is its default — you trace where a
  // flow went; "target" traces where it came from.
  const edgeColor = cfg.edgeColor === "gray" ? GRAY_EDGE : (cfg.edgeColor ?? "source");

  return {
    ...baseOption(),
    // Flows, not categories: read the biggest ones rather than every node.
    aria: ariaDescription({
      kind: "Sankey diagram", metric: cfg.metric, format: (v) => fmt(v),
      points: [...links]
        .sort((a, b) => b.value - a.value)
        .map((l) => ({ name: `${l.source} to ${l.target}`, value: l.value })),
    }),
    // A node has no value of its own, so only edges show one.
    tooltip: {
      trigger: "item",
      ...tooltipSurface(),
      formatter: (p: { dataType?: string; name?: string; value?: unknown }) =>
        (p.dataType === "edge" ? `${p.name}: ${fmt(p.value)}` : String(p.name ?? "")),
    },
    series: [{
      type: "sankey",
      data,
      links,
      nodeAlign: cfg.nodeAlign ?? "left",
      nodeWidth: NODE_WIDTH,
      // Metabase pins the nodes. A sankey's layout IS the answer — which node sits at which depth,
      // and in what order — so letting a pointer drag one apart only ever destroys information.
      draggable: false,
      emphasis: { focus: "adjacency" },
      lineStyle: { color: edgeColor, opacity: EDGE_OPACITY, curveness: CURVENESS },
      label: {
        fontSize: CHART_THEME.axisFontSize, color: ink,
        // A node label lands on top of the edges leaving it. The halo is what keeps it readable
        // there — without it the text competes with whatever flows behind it.
        textBorderWidth: LABEL_HALO, textBorderColor: halo,
      },
      edgeLabel: {
        show: !!cfg.showEdgeLabels,
        fontSize: CHART_THEME.axisFontSize, color: ink,
        textBorderWidth: LABEL_HALO, textBorderColor: halo,
        formatter: (p: { value?: unknown }) => fmt(p.value, labelCompact),
      },
      labelLayout: { hideOverlap: true },
      left: "3%", right: "8%", top: "4%", bottom: "4%",
    }],
  };
}
