import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { formatValue, formatDimensionValue } from "../../format";
import { ariaDescription, baseOption, tooltipSurface } from "../base";
import { CHART_THEME, COLOR_FALLBACK, resolveColor, resolveFontFamily } from "../palette";
import { colorsForKeys } from "../keyColors";
import {
  measureText as defaultMeasureText,
  fitSliceLabel,
  type MeasureText,
} from "../sliceLabel";

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
/** `SANKEY_CHART_STYLE.padding` — the gutter Metabase leaves before it starts reserving label room.
 * Only the RIGHT side uses it here: it is the one side that has to hold text (see `rightGutter`),
 * and a percentage alone collapses to nothing on the narrow card this was measured on — 8% of 166px
 * is 13px, which is why the labels ran off the edge. The floor is what stops that. */
const GUTTER_PERCENT = 0.05;
const GUTTER_MIN = 24;
/** `MAX_RIGHT_LABEL_LENGTH_PERCENT`. A quarter of the card is the most Metabase will surrender to
 * the last column's names; past that the labels are cut to fit rather than the diagram squeezed. */
const MAX_RIGHT_LABEL_PERCENT = 0.25;

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

/**
 * The nodes ECharts will draw in the RIGHTMOST column — the only ones whose labels need room
 * reserved for them, since every label is drawn to the right of its node and the others have the
 * rest of the diagram to spill into. Metabase's `getMostRightNodes`.
 *
 * Two ways to be in that column. Being at the deepest level is the obvious one. The other is
 * Metabase's `shouldIncludeAllNodesWithoutOutputs`: with `nodeAlign` anything but "left", a node
 * with no outgoing flow is pushed to the right EDGE regardless of how deep it actually is, so a
 * one-hop dead end sits beside a five-hop one. Under "left" that same node stays at its own depth
 * with the whole rest of the diagram still to its right, and reserving a gutter for it would give
 * away a quarter of the canvas for a label that was never near the edge.
 *
 * Depth is the LONGEST path from a root, which is what ECharts' own layout uses, computed by a
 * topological sweep. A node still holding an in-degree when the sweep runs dry is on a cycle —
 * `sankeyProblem` refuses those before they reach here, so this only has to not hang.
 */
function rightmostNodeNames(
  names: string[],
  links: { source: string; target: string }[],
  nodeAlign: NodeAlign,
): Set<string> {
  const level = new Map(names.map((n) => [n, 0]));
  const outgoing = new Map<string, string[]>();
  const inDegree = new Map(names.map((n) => [n, 0]));
  for (const l of links) {
    const outs = outgoing.get(l.source);
    if (outs) outs.push(l.target); else outgoing.set(l.source, [l.target]);
    inDegree.set(l.target, (inDegree.get(l.target) ?? 0) + 1);
  }

  const queue = names.filter((n) => inDegree.get(n) === 0);
  for (let i = 0; i < queue.length; i++) {
    const node = queue[i];
    for (const next of outgoing.get(node) ?? []) {
      level.set(next, Math.max(level.get(next) ?? 0, (level.get(node) ?? 0) + 1));
      const remaining = (inDegree.get(next) ?? 0) - 1;
      inDegree.set(next, remaining);
      if (remaining === 0) queue.push(next);
    }
  }

  const maxLevel = Math.max(...level.values());
  const sinksCount = nodeAlign !== "left";
  return new Set(
    names.filter((n) => level.get(n) === maxLevel || (sinksCount && !outgoing.has(n))),
  );
}

// Metabase-style Sankey: weighted flows from source category to target category.
export function buildSankeyOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: SankeyBuildConfig,
  /** Injectable text measurer, defaulting to the real canvas one — the convention `buildPieOption`
   * set, down to the POSITION: measurer fourth, size fifth. The two files are the only ones that
   * take a measurer, so a third builder will copy whichever it opens first, and they must not
   * disagree. (This is what lets the fit below be unit tested without a rasteriser — see
   * sliceLabel.ts. Production callers pass `undefined` and take the default.) */
  measureText: MeasureText = defaultMeasureText,
  /** The chart's measured pixel size, when the caller has one — this builder is pure data with no
   * DOM of its own, so EChartsRenderer measures the container and threads it through (see
   * `buildPieOption`, the first to take one).
   *
   * READ, for the one decision it was threaded in for: how much of the width the LAST column's
   * labels get. ECharts' `label.position` defaults to "right" and this file does not override it,
   * so every label is drawn to the right of its node — including the last column's, which have only
   * the right gutter to live in. That gutter used to be a flat `"8%"`: 13px on the 166px card this
   * was measured on, against a 63px name. The labels started 8px from the edge and ran 30-55px PAST
   * it, and since no `label.width` or `label.overflow` was set anywhere ECharts did no ellipsis of
   * its own — the container edge simply cut the text, leaving the one- and two-character stubs the
   * design spec called truncation (docs/en/specs/2026-08-01-chart-parity-design.md).
   *
   * What happens now is Metabase's `getSankeyLayout`: measure the widest label in the rightmost
   * column, add it to the right gutter, and cap that at a quarter of the width — past which the
   * labels are cut to the cap with a real ellipsis rather than the diagram squeezed any further.
   * `labelLayout.hideOverlap` below stays as the backstop it always was, for labels that collide
   * with each other VERTICALLY; it never had anything to say about running off the right edge.
   *
   * Undefined means the caller has no measurement (unit tests that aren't about fit, and any caller
   * passing a plain option object rather than a size function). Then nothing is fitted: the gutter
   * keeps its percentage and no label is cut against a width nobody supplied — the same safe
   * direction `buildPieOption` takes. */
  size?: EChartsSize,
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
  // Each node states its OWN colour, drawn from egret's accents. Left to itself ECharts INTERPOLATES
  // along the palette array for a sankey rather than picking discrete entries: measured on seven
  // nodes it drew #7EB967, #F3AF6C, #F6BF6A, #CA8AAA, #61A8B5 — five colours that are in no design
  // system, muddied versions sitting between two accents. Metabase's nodes measure as clean
  // accents (#88BF4D, #509EE3, #EF8C8C, #F9D45C, #98D9D9), which is what naming them here gives.
  //
  // WHICH accent each node gets is keyed on its NAME, not on the order it happened to be met in.
  // Two reasons. A flow's nodes are the same categories other cards plot, and a name that keeps its
  // colour across a dashboard can be followed from card to card. And the position it replaces was
  // `seriesColor(i)`, i.e. `i % 8` — so the ninth node was drawn in the first one's colour, on a
  // chart whose ceiling is 150 nodes and whose whole point is telling flows apart. Past eight keys
  // `colorsForKeys` widens the harmony instead of wrapping it.
  const nodeColors = colorsForKeys([...names]);
  const data = [...names].map((name) => ({ name, itemStyle: { color: nodeColors[name] } }));

  /**
   * A NODE'S NAME AS THE LABEL IT IS DRAWN AS — the format this builder read for its metric and
   * never for its two dimensions.
   *
   * The source and target columns reach `FormatSection` (`styleColumnsFor` fell through to
   * `return resultColumns` for a sankey), so both get a Date style / prefix select; `fmt` below
   * resolves `columnSettings?.[cfg.metric]` only, so everything typed into either was stored and
   * drawn nowhere. Measured in the browser on a flow whose source was a month-bucketed
   * `order_month`: the panel read `Date style: January 2024` while every node read `2026-01-01`.
   *
   * WHICH COLUMN'S FORMAT, for a name that appears as both a source and a target, is Metabase's
   * rule copied exactly — `echarts/graph/sankey/model/formatters.ts`:
   * `node = !hasOutputs ? target(rawName) : source(rawName)`. A SINK takes the target column's
   * format; every other node takes the source column's. The two formats can genuinely differ, and
   * upstream accepts the consequence that one value then reads differently at different depths, so
   * guessing a single column here would have been a coin toss with a plausible-looking answer.
   *
   * A FORMATTER, NOT A REWRITE OF THE GRAPH. `data[].name` and `links[].source`/`.target` stay the
   * raw cells: they are the DAG the cycle check walks, the keys `colorsForKeys` assigns accents to,
   * and the strings `ChartRenderer`'s sankey drill matches back to a row to build its filter.
   * Formatting the identity is the drill-on-display-strings defect four chart families have now
   * had, and here it would also mean two raw values that format alike silently merging into one
   * node. So only what a reader sees — the node label, the tooltip, the aria description and the
   * right-gutter measurement — goes through this.
   */
  const hasOutputs = new Set(links.map((l) => l.source));
  const srcType = si != null ? columns[si].type : "varchar";
  const tgtType = ti != null ? columns[ti].type : "varchar";
  const srcFormat = cfg.source != null ? cfg.columnSettings?.[cfg.source] : undefined;
  const tgtFormat = cfg.target != null ? cfg.columnSettings?.[cfg.target] : undefined;
  const nodeLabel = (raw: unknown) => {
    const name = String(raw ?? "");
    return hasOutputs.has(name)
      ? formatDimensionValue(name, cfg.source ?? "", srcType, srcFormat)
      : formatDimensionValue(name, cfg.target ?? "", tgtType, tgtFormat);
  };

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

  // Room for the labels this chart is about to draw — see the `size` parameter's comment.
  const nodeLabelFont = {
    size: CHART_THEME.axisFontSize, weight: CHART_THEME.axisWeight, family: resolveFontFamily(),
  };
  let rightGutter: number | string = "8%";
  let cutTo: number | undefined;
  let cutNodes: Set<string> | undefined;
  if (size != null && size.width > 0 && names.size > 0) {
    const rightmost = rightmostNodeNames([...names], links, cfg.nodeAlign ?? "left");
    // The label as DRAWN, not the raw key: the gutter exists to hold the text ECharts will paint,
    // and "2026-01-01" is ten characters where "January 2026" is twelve. Measuring the key would
    // reserve a gutter the labels then run out of — the exact failure this measurement replaced.
    const widest = Math.max(0, ...[...rightmost].map((n) => measureText(nodeLabel(n), nodeLabelFont)));
    const cap = size.width * MAX_RIGHT_LABEL_PERCENT;
    rightGutter = Math.max(size.width * GUTTER_PERCENT, GUTTER_MIN) + Math.min(cap, widest);
    // Only when the cap actually binds. Below it every label fits the room reserved for it, so
    // there is nothing to cut and no formatter worth carrying.
    if (widest > cap) {
      cutTo = cap;
      cutNodes = rightmost;
    }
  }

  return {
    ...baseOption(),
    // Flows, not categories: read the biggest ones rather than every node.
    aria: ariaDescription({
      kind: "Sankey diagram", metric: cfg.metric, format: (v) => fmt(v),
      points: [...links]
        .sort((a, b) => b.value - a.value)
        .map((l) => ({ name: `${nodeLabel(l.source)} to ${nodeLabel(l.target)}`, value: l.value })),
    }),
    // A node has no value of its own, so only edges show one.
    tooltip: {
      trigger: "item",
      ...tooltipSurface(),
      // A NODE hover is formatted here; an EDGE hover is not, and the asymmetry is ECharts', not a
      // choice. For an edge `p.name` is a string ECharts has already built by joining the two ends
      // with " > ", so there are no raw values left to format — only a joined label to take apart
      // and guess at. `p.data` carries the link, so the ends ARE recoverable, and they are: the
      // edge branch rebuilds the label from them rather than reprinting ECharts' join.
      formatter: (p: { dataType?: string; name?: string; value?: unknown; data?: { source?: unknown; target?: unknown } }) => {
        if (p.dataType !== "edge") return nodeLabel(p.name);
        const ends = p.data?.source != null && p.data?.target != null
          ? `${nodeLabel(p.data.source)} > ${nodeLabel(p.data.target)}`
          : String(p.name ?? "");
        return `${ends}: ${fmt(p.value)}`;
      },
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
        // An ellipsis the reader can SEE, on the labels that would otherwise have been cut by the
        // canvas edge with no mark at all — and only on those, since the other columns' labels have
        // the diagram to spill into. Metabase's `nodeIndicesWithTruncatedLabels`, keyed on the name
        // rather than the index because egret's nodes carry no `value` to key on.
        //
        // `fitSliceLabel` rather than a bare `truncateLabelText`, so the last column inherits the
        // MIN_TRUNCATED_LABEL_CHARS floor its sibling already enforces: below three surviving
        // characters a truncation stops identifying anything and starts reading as a real short
        // value — plenty of genuine categories ARE two-letter codes — so on a card too narrow to
        // cut honestly the label is dropped instead. A name that FITS is untouched however short it
        // is; the floor is about cutting, not about length. `?? ""` because ECharts wants a string.
        //
        // UNCONDITIONAL NOW, where it used to be spread in only when the cap bound. The formatter
        // has a second job — running the node's name through its own column format (`nodeLabel`) —
        // and that job applies to every node whether or not anything is being cut. The truncation
        // is applied to the FORMATTED label, which is the only order that can be right: the cap is
        // a pixel width and the pixels belong to the text being drawn.
        formatter: (p: { name?: unknown }) => {
          const label = nodeLabel(p.name);
          if (cutNodes == null || cutTo == null || !cutNodes.has(String(p.name ?? ""))) return label;
          return fitSliceLabel({
            label, availableSpace: cutTo, showLabels: true,
            measureText, fontStyle: nodeLabelFont,
          }) ?? "";
        },
      },
      edgeLabel: {
        show: !!cfg.showEdgeLabels,
        fontSize: CHART_THEME.axisFontSize, color: ink,
        textBorderWidth: LABEL_HALO, textBorderColor: halo,
        formatter: (p: { value?: unknown }) => fmt(p.value, labelCompact),
      },
      // The vertical backstop, and the only one of the two mechanisms that is not this builder's.
      // The gutter above governs HORIZONTAL fit; this governs nodes stacked so close that their
      // names run through each other, which no gutter can help with.
      //
      // It was tree-shaken out of production builds until `register.ts` listed `LabelLayout`.
      // Measured on restoring it: BELOW 260px WIDE it drops three or four of seven node labels
      // (five to seven of eleven on a busier diagram), and at 260px and above nothing changes. Each
      // drop is a real rectangle intersection between two names, hand-checked — it is not undoing
      // the gutter's decision, which is about the right edge and says so above. A card that narrow
      // was previously drawing those names on top of one another.
      labelLayout: { hideOverlap: true },
      // `right` is the only side that has to hold text — the first column's labels are drawn INTO
      // the diagram, so its gutter costs nothing.
      left: "3%", right: rightGutter, top: "4%", bottom: "4%",
    }],
  };
}
