import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseOption, itemTooltip, bottomLegend } from "../base";
import { OTHER_SLICE_COLOR, COLOR_FALLBACK, resolveColor, seriesColor, lightenOklch, getTextColorForBackground } from "../palette";
import {
  calcAvailableDonutSliceLabelLength,
  calcInnerOuterRadiusesForRing,
  fitSliceLabel,
  measureText as defaultMeasureText,
  type MeasureText,
} from "../sliceLabel";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";

export interface ResultColumn { name: string; type: string }
export interface PieBuildConfig {
  /** Up to three, drawn as nested rings from the centre out: dimensions[0] is the
   * innermost ring, each subsequent entry nests inside the previous one's slices. */
  dimensions?: string[];
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  /** Slices below this percent of the total are grouped into one "Other" slice. Default 2.5; 0 disables. */
  sliceThreshold?: number;
  /** Sort slices by value descending. Default true. */
  sortRows?: boolean;
  /** Donut only: show the grand total in the centre. */
  showTotal?: boolean;
  /** Whether the slice's percent appears in its label. Default "off". */
  percentVisibility?: "off" | "inside" | "both";
  /** Per-slice colour override, keyed by the slice (dimension) value. */
  sliceColors?: Record<string, string>;
  /** Curation of the FIRST dimension's slices only (ring 0) — matches Metabase's `pie.rows`.
   * Extending this to inner rings would need a compound key and isn't worth it for rings that
   * exist to be scanned, not curated.
   *
   * All three key on the slice's RAW dimension value, never its renamed display text — so a
   * rename never breaks a colour override, a hide, or a manual position.
   *
   * `hiddenSlices` rows are dropped before the ring's total/threshold is computed at all, so
   * the remaining slices' shares re-base as if the hidden category never existed; an outer
   * ring's rows nested under a hidden parent are dropped with it (the same orphan-drop already
   * used for a parent lost to the sign/zero rule), not folded into anything and not kept alone.
   *
   * `sliceOrder` is a display-order override: unlisted values keep their natural (post-sort)
   * order after the named ones — the same rule `CartesianConfig.seriesOrder` uses. */
  sliceNames?: Record<string, string>;
  hiddenSlices?: string[];
  sliceOrder?: string[];
  /** Decimal places shown on slice percentages (label and tooltip). Default 2. */
  decimals?: number;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  /** Temporal extraction unit per dimension, keyed by column name (e.g. `{ region: "day_of_week" }`) —
   * the same shape as `params.spec.temporal_units`, so ChartRenderer forwards it unmodified and each
   * ring looks up its own dimension's unit. */
  dimensionUnits?: Record<string, string>;
}

const OTHER_NAME = "Other";
const DEFAULT_THRESHOLD = 2.5; // percent — Metabase SLICE_THRESHOLD (0.025)
// Sentinel identifying "this ring's own Other bucket" as a reorder group, distinct from any real
// column value. Collides only if a real dimension value were literally this string (same accepted
// risk as OTHER_NAME itself already colliding with a real "Other" category value).
const OTHER_KEY = "\u0000__other__\u0000";
// Joins a dimension-value tuple into one aggregation key. A control character keeps it well clear
// of ordinary column values (still not bulletproof against a value containing it, same class of
// accepted risk as OTHER_KEY/OTHER_NAME).
const KEY_SEP = "␟";

/** One (aggregated) slice going into a ring, still carrying the identity fields ring-nesting needs:
 * `key` — this ring's own aggregation key (the dimension-value path up to and including this ring);
 * `parentKey` — the same path one ring in, i.e. the key this slice's row(s) aggregate to in the
 * next ring inward. Absent on the synthetic "Other" slice, which has no single origin. */
interface Slice { name: string; value: number; key?: string; parentKey?: string; itemStyle?: { color: string } }

/** A ring's raw (pre-robustness-rules) aggregate: rows grouped by the dimension-value tuple up to
 * and including this ring's own dimension, values summed. Pre-existing behaviour, unchanged by
 * nesting: grouping is by DISPLAYED value, so `null`, `undefined` and `""` all render as the same
 * `""`-named slice and merge together. */
function aggregateRing(rows: unknown[][], dimIdxs: (number | undefined)[], units: (string | undefined)[], mi: number): Slice[] {
  const groups = new Map<string, Slice>();
  for (const r of rows) {
    const parts = dimIdxs.map((di, i) => {
      const raw = di != null ? r[di] : undefined;
      return isExtractUnit(units[i]) ? temporalLabel(units[i], raw) : String(raw ?? "");
    });
    const key = parts.join(KEY_SEP);
    const value = Number(r[mi]);
    const existing = groups.get(key);
    if (existing) existing.value += value;
    else groups.set(key, {
      key,
      parentKey: parts.length > 1 ? parts.slice(0, -1).join(KEY_SEP) : undefined,
      name: parts[parts.length - 1],
      value,
    });
  }
  return [...groups.values()];
}

// Identifies a specific parent's own "Other" bucket, distinct from every other parent's, so two
// different parents' folded buckets never collide as the same map key downstream (groupOf, and
// the colour-anchor pass's per-ring anchor map — see buildPieOption). Ring 0's items all carry
// `parentKey: undefined` (one implicit parent, the whole chart), so this reduces to the plain
// OTHER_KEY sentinel there — unchanged from before this fix.
function otherKeyFor(parentKey: string | undefined): string {
  return parentKey != null ? `${OTHER_KEY}${KEY_SEP}${parentKey}` : OTHER_KEY;
}

// Clean-room of Metabase's pie data prep (pie/model/index.ts): drop zeros, handle
// negatives (all-negative → abs, mixed → drop negatives), sort by value desc, then
// fold sub-threshold slices into a muted "Other" — Runs once per ring — the outer
// rings feed it their own aggregated (name, value) pairs instead of raw rows, but every
// rule above applies identically and independently at every level.
//
// The Other fold itself is judged PER PARENT, not across the whole ring (Metabase's own model —
// pie/model/index.ts's aggregateChildrenSlices — folds within `node.children`, i.e. per parent,
// for exactly this reason): ring 0's items have no parentKey (one implicit parent, the whole
// chart), so grouping by parentKey there degenerates to the old ring-wide behaviour — a
// single-ring pie is untouched. A ring below 0 has one real parentKey per item (aggregateRing
// always gives these a defined parentKey), so each parent's slices are judged against THAT
// PARENT's own total and, if it has 2+ sub-threshold slices, get THEIR OWN "Other" bucket —
// instead of a chart-wide Other, scattered across every parent, that belongs to none of them
// (the bug this fixes: with 36 outer slices each ~2.8% of the RING but a healthy share of their
// own small parent group, the old ring-wide judgment folded about half of them together with no
// parent to sit under).
//
// Also returns `groupOf`: for every item that survived the finite/sign filter, the key of
// the ring's OWN slice it ended up as — either its own `key` (kept) or that parent's own
// other-key (folded, see otherKeyFor). An item absent from `groupOf` was dropped outright
// (non-finite, zero, or mixed-sign negative) — the next ring inward uses that to drop its now-
// parentless children too.
function buildSlices(items: Slice[], cfg: PieBuildConfig): { slices: Slice[]; groupOf: Map<string, string> } {
  let slices: Slice[] = items.filter((s) => Number.isFinite(s.value));

  const allNegative = slices.length > 0 && slices.every((s) => s.value < 0);
  slices = allNegative
    ? slices.map((s) => ({ ...s, value: Math.abs(s.value) }))
    : slices.filter((s) => s.value > 0); // drops zeros and (mixed-sign) negatives

  if (cfg.sortRows !== false) {
    slices = [...slices].sort((a, b) => b.value - a.value);
  }

  const groupOf = new Map<string, string>();
  for (const s of slices) if (s.key) groupOf.set(s.key, s.key);

  const threshold = (cfg.sliceThreshold ?? DEFAULT_THRESHOLD) / 100;
  if (threshold > 0 && slices.length > 0) {
    const parentGroups = new Map<string | undefined, Slice[]>();
    for (const s of slices) {
      const group = parentGroups.get(s.parentKey);
      if (group) group.push(s);
      else parentGroups.set(s.parentKey, [s]);
    }

    const result: Slice[] = [];
    for (const group of parentGroups.values()) {
      const groupTotal = group.reduce((t, s) => t + s.value, 0);
      const small = group.filter((s) => s.value / groupTotal < threshold);
      // Only worth an "Other" bucket when it merges 2+ slices WITHIN THIS PARENT (matches
      // Metabase's per-parent merge-2+ rule).
      if (small.length > 1) {
        const big = group.filter((s) => s.value / groupTotal >= threshold);
        const otherTotal = small.reduce((t, s) => t + s.value, 0);
        const otherColor = resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel);
        const parentKey = group[0].parentKey;
        const otherKey = otherKeyFor(parentKey);
        for (const s of small) if (s.key) groupOf.set(s.key, otherKey);
        result.push(
          ...big,
          { name: OTHER_NAME, value: otherTotal, key: otherKey, parentKey, itemStyle: { color: otherColor } },
        );
      } else {
        result.push(...group);
      }
    }
    slices = result;
  }

  return { slices, groupOf };
}

/** Places a ring's slices parent-by-parent, in the parent ring's own finalized order — required
 * because ECharts draws a pie series in array order starting at `startAngle`, so a child ring's
 * data must already be grouped under its parent's arc rather than sorted across parents.
 *
 * Grouped by KEY, not by display name: two different parents can have children with the same
 * name (e.g. an "Accessories" category under two different regions), and grouping by name would
 * wrongly merge them.
 *
 * A slice whose parent got folded into the parent's own "Other" (parentGroupOf maps its
 * parentKey to that parent's own other-key, see otherKeyFor) is grouped under that parent's
 * Other position instead of vanishing.
 * A slice with no resolvable group is placed last, after every parent group — defensive only:
 * every level > 0 item, including each parent's own "Other" bucket, carries a defined parentKey
 * (buildSlices folds within parents, not across the ring), so this path isn't expected to trigger
 * there; it exists purely as a safe fallback rather than a silent drop.
 */
function reorderByParent(slices: Slice[], parentGroupOf: Map<string, string>, parentOrder: string[]): Slice[] {
  const groups = new Map<string, Slice[]>();
  const ungrouped: Slice[] = [];
  for (const s of slices) {
    const groupKey = s.parentKey != null ? parentGroupOf.get(s.parentKey) : undefined;
    if (groupKey == null) { ungrouped.push(s); continue; }
    const arr = groups.get(groupKey);
    if (arr) arr.push(s); else groups.set(groupKey, [s]);
  }
  const ordered: Slice[] = [];
  for (const key of parentOrder) ordered.push(...(groups.get(key) ?? []));
  ordered.push(...ungrouped);
  return ordered;
}

export function buildPieOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: PieBuildConfig,
  // Injectable text measurer — defaults to a real canvas measurer (see sliceLabel.ts); tests
  // pass a deterministic stub instead of depending on a real text rasteriser (see that file's
  // own header comment on why this has to be threaded through as a plain callback here rather
  // than a renderingContext object like Metabase's).
  measureText: MeasureText = defaultMeasureText,
  // The chart's REAL rendered pixel size — this builder is pure data with no DOM access of its
  // own, so it can't measure its own container; EChartsRenderer measures it (ResizeObserver) and
  // threads it through by calling this function itself rather than a plain option object (see
  // that file). Undefined (no caller-supplied size — most unit tests that aren't testing label
  // fit at all) means "unknown": PX_PER_RADIUS_PERCENT below then collapses every ring's inner
  // radius to 0px, which takes the SAME "true full pie" path this file already uses when there's
  // genuinely no donut hole (see the comment on `availableSpace` further down) — the per-slice
  // fit check is skipped entirely and every label is shown in full, backstopped only by
  // `labelLayout.hideOverlap`. That's the safe direction for "we don't know the real size": it
  // never falsely truncates against a guessed number that could just as easily be wrong, the
  // same reasoning that ruled out NOMINAL_CONTAINER_MIN_DIMENSION_PX in the first place.
  containerSize?: { width: number; height: number },
): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const dims = cfg.dimensions ?? [];
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  // One entry per ring, innermost first. Aggregate level L groups rows by dimensions[0..L] (so the
  // innermost ring is coarsest, summing over every dimension nested inside it), run that ring's own
  // slice through buildSlices, then reorder it under its parent ring's finalized order.
  const rings: Slice[][] = [];
  // groupOfByLevel[L] is ring L's OWN "did this key survive or fold into Other" map (buildSlices'
  // `groupOf`), kept around after the loop so the colour-inheritance pass below can resolve a
  // ring L+1 slice's parentKey the same way reorderByParent does — through its parent's fold state,
  // not just its raw identity.
  const groupOfByLevel: Map<string, string>[] = [];
  if (mi != null && dims.length > 0) {
    let parentGroupOf: Map<string, string> | undefined;
    let parentOrder: string[] = [];
    for (let level = 0; level < dims.length; level++) {
      const path = dims.slice(0, level + 1);
      const dimIdxs = path.map((d) => idx[d]);
      const units = path.map((d) => cfg.dimensionUnits?.[d]);
      let items = aggregateRing(rows, dimIdxs, units, mi);
      // A row whose ancestor got dropped outright (not even folded to Other) at the parent ring has
      // no home here either: its parent's slice doesn't exist to nest under.
      if (level > 0 && parentGroupOf) {
        items = items.filter((it) => it.parentKey != null && parentGroupOf!.has(it.parentKey));
      }
      // Ring 0 only (task 6): a hidden slice's rows are dropped BEFORE buildSlices sees them,
      // so its value never enters the total or the Other-threshold computation — the rest of
      // the ring re-bases as if the hidden category never existed. This also means `groupOf`
      // below never gets an entry for it, so the orphan-drop above removes its children in the
      // next ring out for free, the same way it already handles a parent lost to the sign/zero
      // rule.
      if (level === 0 && cfg.hiddenSlices?.length) {
        items = items.filter((it) => !cfg.hiddenSlices!.includes(it.name));
      }
      const { slices, groupOf } = buildSlices(items, cfg);
      let ordered = level === 0 ? slices : reorderByParent(slices, parentGroupOf!, parentOrder);
      // Ring 0 only: sliceOrder is a display-order override keyed on the same raw value as
      // hiddenSlices — applied AFTER buildSlices (so it can't influence the Other threshold),
      // and BEFORE parentOrder is captured just below, so an outer ring's contiguous grouping
      // (reorderByParent, for level > 0) follows this same custom order.
      if (level === 0 && cfg.sliceOrder?.length) {
        const rank = new Map(cfg.sliceOrder.map((n, i) => [n, i]));
        ordered = [...ordered].sort(
          (a, b) => (rank.get(a.name) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.name) ?? Number.MAX_SAFE_INTEGER),
        );
      }
      rings.push(ordered);
      groupOfByLevel.push(groupOf);
      parentGroupOf = groupOf;
      parentOrder = ordered.map((s) => s.key ?? OTHER_KEY);
    }
  }
  const effectiveRings = rings.length > 0 ? rings : [[]];

  // Per-slice colour overrides win over the palette slot (the "Other" slice keeps its muted tint).
  // Matched by NAME on every ring, not just ring 0 — so a value that happens to appear as a
  // dimension value in more than one ring (e.g. the same string used for both a region and a
  // category) takes the override in both places it appears. Pre-existing behaviour, unchanged
  // here; flagged because it's easy to assume "override" means "ring-0 only".
  //
  // The override goes through `resolveColor` like every other colour in this file: it arrives
  // from the chart's settings JSONB, which is never whitelisted server-side, so it can be
  // hand-written, migrated or simply mistyped. `resolveColor` is the one place that validates
  // a colour from a less trusted source — anything the renderer can't draw comes back as the
  // fallback, and an empty-string fallback (not a colour, so unambiguous) turns that into "no
  // override": the slice keeps its palette slot instead of being painted with a value ECharts
  // would ignore and `getTextColorForBackground` would then silently mis-contrast against.
  if (cfg.sliceColors) {
    for (const ring of effectiveRings) {
      for (const s of ring) {
        const c = cfg.sliceColors[s.name];
        if (!c || s.name === OTHER_NAME) continue;
        const resolved = resolveColor(c, "");
        if (resolved) s.itemStyle = { color: resolved };
      }
    }
  }

  // Ring n's slice colour: its family's own BASE colour (an implicit palette slot, or a
  // sliceColors override — on whichever ring introduced it) lightened by a single
  // `lightenOklch(base, steps)` call, never by re-lightening an already-lightened colour.
  // Chaining ring-over-ring (each hop calling lightenOklch on the PREVIOUS ring's already
  // 8-bit-rounded output) was tried first and rejected: every hop that hits the clamp
  // (see OKLCH_MAX_L) rounds to hex and re-parses, so the per-channel gamut clipping compounds
  // and a pale colour can collapse to indistinguishable-from-white by ring 3 (verified: e.g.
  // #EF8C8C's chroma fell from 0.121 to 0.0000000373 after 3 chained hops). A single call from
  // a stable base has no such compounding.
  //
  // `anchors` maps a ring's own key (or OTHER_KEY for its synthetic "Other" bucket) to
  // { base, sinceLevel }: the colour to lighten from, and the ring level it was FIXED at. Ring 0
  // fixes every slice's base at level 0 (sinceLevel: 0), so ring n's slice ordinarily computes
  // `lightenOklch(base, n)` directly off the ring-0 colour. Two things reset the anchor instead
  // of propagating it unchanged:
  //  - a ring's own synthetic "Other" bucket already has a fixed muted colour (set above,
  //    unconditionally, at every ring depth) — it becomes a fresh anchor at ITS level, so
  //    anything nested under it lightens from Other's muted colour, not from whatever palette
  //    slot its constituent (folded-away) parents would have had;
  //  - a sliceColors override on a ring below 0 (see the note above — the same NAME can match
  //    more than one ring) is itself a fresh anchor at that level, for the same reason.
  let anchors = new Map<string, { base: string; sinceLevel: number }>();
  (effectiveRings[0] ?? []).forEach((s, i) => {
    anchors.set(s.key ?? OTHER_KEY, { base: s.itemStyle?.color ?? seriesColor(i), sinceLevel: 0 });
  });
  for (let level = 1; level < effectiveRings.length; level++) {
    const parentFold = groupOfByLevel[level - 1];
    const nextAnchors = new Map<string, { base: string; sinceLevel: number }>();
    for (const s of effectiveRings[level]) {
      const groupKey = s.parentKey != null ? parentFold?.get(s.parentKey) : undefined;
      const ancestorAnchor = groupKey != null ? anchors.get(groupKey) : undefined;
      const alreadyStyled = s.itemStyle != null; // this ring's own Other bucket, or its own sliceColors override
      if (!alreadyStyled && ancestorAnchor) {
        s.itemStyle = { color: lightenOklch(ancestorAnchor.base, level - ancestorAnchor.sinceLevel) };
      }
      const key = s.key ?? OTHER_KEY;
      if (alreadyStyled) {
        nextAnchors.set(key, { base: s.itemStyle!.color, sinceLevel: level });
      } else if (ancestorAnchor) {
        nextAnchors.set(key, ancestorAnchor); // propagate the SAME anchor, not this ring's own colour
      } else {
        // Defensive only: every non-Other slice's parentKey should resolve via groupOf (orphans
        // are filtered out earlier in the main loop) — a muted grey is a safe, visible fallback.
        nextAnchors.set(key, { base: resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel), sinceLevel: level });
      }
    }
    anchors = nextAnchors;
  }

  // Ring 0 only, and deliberately LAST: a display rename must not disturb anything computed
  // above by raw value — the sliceColors match (by name, above) and the colour-inheritance
  // anchors (by key, just above) have already run against the raw value, so renaming now only
  // changes what's shown, never what's found by hiddenSlices/sliceOrder/sliceColors.
  if (cfg.sliceNames) {
    for (const s of effectiveRings[0] ?? []) {
      const renamed = cfg.sliceNames[s.name];
      if (renamed) s.name = renamed;
    }
  }

  const dimension = dims[0];
  const data = effectiveRings[0];
  const ringCount = effectiveRings.length;
  const multiRing = ringCount > 1;

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  // Label: name (showLabels) and/or percent (percentVisibility) — Metabase getSliceLabel. Applied
  // the same way to every ring. Defaults resolve here, at render time, never in stored settings
  // (this program's standing rule) — `??` so an explicit `false` stays off; `||` would treat a
  // saved `false` the same as "unset" and never let anyone turn labels off.
  const showName = cfg.showLabels ?? true;
  const showPercent = cfg.percentVisibility === "inside" || cfg.percentVisibility === "both";
  const decimals = cfg.decimals ?? 2;
  const pct = (p: number) => `${parseFloat(p.toFixed(decimals))}%`; // configurable decimals, default 2 like Metabase
  const labelFormatter = (p: { name: string; percent: number; data?: { parentPercent?: number } }) => {
    const name = showName ? p.name : undefined;
    // ECharts' own `percent` is share-of-that-series (share of the ring); an outer-ring datum
    // carries a parent-relative override (parentPercent, computed per-slice below) that wins
    // when present. Ring 0 never sets it, so this is a no-op there.
    const percentValue = p.data?.parentPercent ?? p.percent;
    const percent = showPercent ? pct(percentValue) : undefined;
    if (name != null && percent != null) return `${name}: ${percent}`;
    return name ?? percent ?? "";
  };

  // Slice label sizing — Metabase's constants.ts: multiRingFontSize (12) once there's more than
  // one ring; a single ring gets Metabase's maxFontSize/minFontSize (20/14) RESPONSIVE to the
  // rendered pixel side length, computed there from an actual container measurement this pure
  // data-only builder doesn't have (no DOM access here — see EChartsRenderer, which never passes
  // its size in). Rather than fake a measurement, this uses Metabase's FLOOR (minFontSize, 14):
  // it never renders LARGER than Metabase would at any size, which is the safe direction — a
  // label too big is what causes more overlap/hiding, not too small. fontWeight 700 either way
  // (slice.label.fontWeight) and drawn ON the slice (`position: "inside"`), matching what makes
  // Metabase's sunburst readable without a legend.
  const sliceFontSize = multiRing ? 12 : 14;

  // ECharts has no built-in shrink-or-truncate for a label that doesn't fit its slice (unlike
  // Metabase's sunburst, which measures text against the slice's own arc length and truncates or
  // drops it). What it DOES have is `labelLayout.hideOverlap`: a post-layout pass that hides
  // whichever labels collide, keeping the rest legible — the same graceful-degradation Metabase
  // reaches for (`labelLayout: { hideOverlap: true }` in its own option.ts). A tiny slice's label
  // disappears instead of overlapping into mush; it is still reachable via the tooltip.
  const labelLayout = { hideOverlap: true };

  // Donut centre total — Metabase getTotalGraphicOption (static total; no hover swap yet). The
  // innermost ring's total is the grand total: it's summed over every outer dimension already.
  //
  // Font sizes are Metabase's total.* constants. The three-ring hole (1/4 of the outer edge,
  // below) is tight enough that the regular valueFontSize (22) risks overflowing it, so that
  // case alone drops to Metabase's own escape hatch, valueFontSizeSm (17); one and two rings keep
  // the regular size. Metabase also bolds its "Total" caption (total.fontWeight 700) — the
  // caption text here previously left fontWeight unset (a plain, unbolded caption); now matches.
  const total = data.reduce((t, s) => t + s.value, 0);
  const totalValueFontSize = ringCount >= 3 ? 17 : 22;
  const graphic = cfg.donut && cfg.showTotal && data.length > 0
    ? [
        { type: "text", left: "center", top: "43%", style: { text: fmt(total), fontSize: totalValueFontSize, fontWeight: 700, fill: resolveColor("var(--foreground)", COLOR_FALLBACK.text), textAlign: "center" } },
        { type: "text", left: "center", top: "53%", style: { text: "Total", fontSize: 14, fontWeight: 700, fill: resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel), textAlign: "center" } },
      ]
    : undefined;

  // Radii: Metabase's own ratios (constants.ts) — one ring 3/5, two rings 2/5, three rings 1/4 of
  // the outer radius. Applied on egret's existing 0-70% "outer edge" scale (unchanged) rather
  // than literally re-deriving Metabase's own side-length maths, which this file has no
  // equivalent inputs for.
  //
  // ONE ring is the deliberate exception: it keeps egret's OWN existing donut on/off semantics
  // exactly as before (donut:false -> 0%, donut:true -> 45%) rather than Metabase's 3/5 (42%,
  // on this scale) — moving that hole would violate the "a one-ring pie stays visually
  // unchanged" guarantee for a chart the brief never identified as unreadable in the first
  // place. TWO OR MORE rings is where the ratio is unconditional: there is no sensible
  // multi-ring pie with no hole at all (task spec), so it always applies regardless of `donut`.
  const outerEdge = 70;
  const HOLE_RATIO: Record<number, number> = { 2: 2 / 5, 3: 1 / 4 };
  const holeStart = multiRing ? outerEdge * HOLE_RATIO[Math.min(ringCount, 3)] : (cfg.donut ? 45 : 0);
  const fmtPct = (v: number) => `${parseFloat(v.toFixed(2))}%`;

  // calcAvailableDonutSliceLabelLength (sliceLabel.ts, ported from Metabase — see fix 1b below)
  // needs a real PIXEL radius to compare against a real pixel fontSize. ECharts' own `radius`
  // here is only ever handed the abstract 0-100 percent-of-container units above (`outerEdge`,
  // `holeStart`) — converting those to real pixels needs the chart's actual rendered size, which
  // is exactly what `containerSize` (this function's own parameter, threaded through from
  // EChartsRenderer's ResizeObserver — see that file) supplies. ECharts defines 100% radius as
  // half the container's SHORTER side, so this mirrors that convention with `Math.min` rather
  // than assuming a square container.
  //
  // A PREVIOUS version of this fix used a fixed, guessed 300px reference container here instead
  // of a real measurement (NOMINAL_CONTAINER_MIN_DIMENSION_PX) — reasoning that erring toward a
  // SMALLER assumed container only makes truncation MORE likely, never causes an overlap a real
  // (likely larger) container wouldn't have shown. That reasoning was wrong in practice: a real
  // chart card is routinely ~500-something px, nearly double the guess, so EVERY label was
  // measured against roughly half the room it actually had and truncated far more aggressively
  // than the real render needed — "LATAM" down to "L…", some slices to nothing at all. A wrong
  // guess is wrong in whichever direction it happens to be wrong, which is why this now takes
  // the real size instead of inventing one: no fixed number is safe at every container size a
  // chart can actually render at (a dashboard tile, a full-page card, a phone).
  //
  // No `containerSize` at all (most unit tests below, which aren't testing label-fit geometry)
  // collapses to 0 here, which zeroes every ring's inner/outer px and — per the `availableSpace`
  // comment further down — takes the same "skip the fit check, show labels in full" path as a
  // true full pie with no donut hole. Safe: unknown size means "don't guess," not "guess small."
  const PX_PER_RADIUS_PERCENT = containerSize ? Math.min(containerSize.width, containerSize.height) / 2 / 100 : 0;

  // Metabase's DIMENSIONS.slice.label.padding (pie/constants.ts) — a small margin subtracted
  // from the geometric space a label is allowed, so text doesn't butt right up against a
  // slice's own border.
  const SLICE_LABEL_PADDING = 2;

  // Multi-ring labels run RADIAL (along the radius), not horizontal — fix 1a. A horizontal
  // label needs a wide chord to sit in; a radial one only needs to fit along the ring's own
  // width, which is what makes a narrow multi-ring slice able to hold a label at all. A single
  // ring keeps the previous horizontal behaviour untouched (Metabase's own rule:
  // `chartModel.numRings > 1 ? "radial" : "horizontal"`).
  const labelsPosition = multiRing ? ("radial" as const) : ("horizontal" as const);

  // The two candidate label colours — WCAG AA contrast (fix 2): resolved once, not per slice,
  // since resolveColor's own cache would otherwise do this same var() lookup on every slice.
  // Reads the theme's own tokens (light theme: dark-on-light; dark theme: light-on-dark), same
  // as everything else in this file that reads a CSS custom property — but only because
  // resolveColor's cache is dropped on a theme change and EChartsRenderer then re-applies the
  // option. Neither is true "by construction": without both, these two resolve exactly once per
  // page load and a light<->dark toggle leaves every label on the previous theme's ink.
  const labelTextDark = resolveColor("var(--foreground)", COLOR_FALLBACK.text);
  const labelTextLight = resolveColor("var(--background)", COLOR_FALLBACK.textInverse);

  // Slice gaps and ring borders — Metabase's constants.ts borderProportion/twoRingBorderWidth/
  // threeRingBorderWidth. Metabase fakes its gap with an opaque, background-coloured border
  // because its sunburst series has no native gap; ECharts' `pie` series (this file's series
  // type) DOES support `padAngle` (added 5.4, confirmed present on PieSeriesOption in the
  // installed echarts@6.1.0) — a real, resolution-independent angular gap — so that's used
  // directly for "a 1-degree gap between slices" instead of Metabase's arc-length pixel hack.
  // `padAngle` alone is enough for one ring. Two or more rings ALSO get Metabase's fixed
  // borderWidth (1 for two rings, 0.3 for three), which separates adjacent RINGS from each
  // other radially — something padAngle, being purely angular, cannot do — coloured to match the
  // chart's own background (the card a chart renders inside, per shadcn's Card `bg-card`) rather
  // than a hardcoded white, so the gap disappears into dark mode too.
  const ringBorder = multiRing
    ? { borderColor: resolveColor("var(--card)", COLOR_FALLBACK.card), borderWidth: ringCount === 2 ? 1 : 0.3 }
    : undefined;

  // REVERSED from an earlier decision. This used to compute a slice's percentage as its share of
  // THAT RING's own total everywhere, reasoning that a pie is about parts of a whole and that
  // this kept every ring's percentages comparable with the inner ring's. Seen on a real
  // three-ring chart, that reasoning holds for ring 0 (which has no parent, so "share of the
  // ring" already IS "share of the whole") but breaks down on every ring outside it: a channel
  // slice worth 500 out of its own (region, product) parent's 1000 read as "500/12000 = 4.17% of
  // everything" — arithmetically true, and useless to whoever is looking at that one wedge. What
  // they actually want is "50% of its region-and-product", i.e. share of its PARENT slice, which
  // is what Metabase computes (`normalizedPercentage: otherTotal / node.value`) and what rings 1+
  // now show here too.
  //
  // ECharts computes `percent` once per SERIES (i.e. per ring), with no per-datum override, so
  // there's no way to ask it for "this slice's share of its parent" directly. Instead, each
  // level > 0 slice gets its parent-relative percentage precomputed here and stashed on the datum
  // itself as `parentPercent` (rounded to `decimals`, same as `pct` below); the label formatter
  // above and the shared tooltip formatter (base.ts' itemTooltip) both prefer it over ECharts' own
  // `percent` whenever a datum carries one. Ring 0 never gets one, so both formatters fall back to
  // ECharts' own per-ring percent there — unchanged, since ring 0's share of its ring already is
  // its share of the whole chart.
  //
  // A slice's "own parent" is resolved the same way reorderByParent already resolves it: its
  // `parentKey` (the previous ring's raw aggregation key), folded through that ring's OWN
  // `groupOf` (groupOfByLevel[level - 1]) to the actual key of the previous ring's finalized
  // slice — so a slice nested under a parent that itself folded into that parent ring's own
  // "Other" bucket is correctly treated as a share of that Other bucket's total, not of a parent
  // that no longer exists as its own slice. This is exactly why a ring's own "Other" bucket
  // reports a share of the SAME parent as its folded-in siblings: it carries that same
  // `parentKey`, so it goes through the identical lookup.
  const ringValueByKey = effectiveRings.map((ring) => {
    const map = new Map<string, number>();
    for (const s of ring) map.set(s.key ?? OTHER_KEY, s.value);
    return map;
  });
  const series = effectiveRings.map((ringData, level) => {
    const parentFold = level > 0 ? groupOfByLevel[level - 1] : undefined;
    const parentValues = level > 0 ? ringValueByKey[level - 1] : undefined;

    const { inner: ringInnerPct, outer: ringOuterPct } = calcInnerOuterRadiusesForRing(
      holeStart,
      outerEdge,
      ringCount,
      level + 1,
    );
    const ringInnerPx = ringInnerPct * PX_PER_RADIUS_PERCENT;
    const ringOuterPx = ringOuterPct * PX_PER_RADIUS_PERCENT;
    const fontStyle = { size: sliceFontSize, weight: 700 };

    // Fix 1b: each slice's label is measured against ITS OWN slice's available space and
    // truncated or dropped, rather than relying solely on labelLayout.hideOverlap's post-layout
    // backstop. Needs this slice's own angular span, which nothing upstream computes yet —
    // derived here from a running cumulative share of THIS RING's own total, the same total
    // ECharts itself angles these slices by (each ring is its own "pie" series, starting at the
    // default startAngle/12-o'clock and going clockwise — the same convention
    // calcAvailableDonutSliceLabelLength's geometry assumes).
    const ringTotal = ringData.reduce((t, s) => t + s.value, 0);
    let cumulativeValue = 0;
    const fits = ringData.map((s, i) => {
      const startAngle = ringTotal > 0 ? (cumulativeValue / ringTotal) * 2 * Math.PI : 0;
      cumulativeValue += s.value;
      const endAngle = ringTotal > 0 ? (cumulativeValue / ringTotal) * 2 * Math.PI : 0;

      let percentValue: number;
      if (level > 0 && parentFold && parentValues && s.parentKey != null) {
        const parentFoldKey = parentFold.get(s.parentKey);
        const parentValue = parentFoldKey != null ? parentValues.get(parentFoldKey) : undefined;
        percentValue = parentValue ? (s.value / parentValue) * 100 : ringTotal > 0 ? (s.value / ringTotal) * 100 : 0;
      } else {
        percentValue = ringTotal > 0 ? (s.value / ringTotal) * 100 : 0;
      }
      // Same combination rule as labelFormatter (name +/- percent), called directly with this
      // slice's own values rather than through ECharts' callback — this needs the full,
      // UNtruncated text up front, to measure and fit before rendering even starts.
      const fullText = labelFormatter({ name: s.name, percent: parseFloat(percentValue.toFixed(decimals)) });

      // calcAvailableDonutSliceLabelLength is, as its name says, a DONUT formula — with a true
      // zero inner radius (egret's own "one ring, donut off" case; see the "a one-ring pie stays
      // visually unchanged" guarantee on `outerEdge`/`holeStart` above) its inner chord is
      // degenerate (a single point), which would make EVERY slice's available space collapse to
      // 0 and drop every label outright — a real regression for egret's plain, no-hole pie,
      // which the fix's own defect write-up never identified as a problem (it's about a 3-ring
      // SUNBURST, always a donut). Metabase never hits this because its own ratios always leave
      // some hole, even at one ring (innerRadiusRatio 3/5). So a true full pie skips this fit
      // check entirely and keeps showing full labels, same as before this fix — hideOverlap
      // remains its sole backstop, exactly as it already was.
      const availableSpace =
        ringInnerPx > 0
          ? calcAvailableDonutSliceLabelLength(ringInnerPx, ringOuterPx, startAngle, endAngle, sliceFontSize, labelsPosition) -
            2 * SLICE_LABEL_PADDING
          : Infinity;

      const displayLabel = fitSliceLabel({
        label: fullText,
        availableSpace,
        showLabels: showName,
        measureText,
        fontStyle,
      });

      // Fix 2: this slice's OWN render colour — its explicit override/inherited-lightened
      // colour if it has one, or (matching what ECharts itself would assign) the palette colour
      // at this slice's own index in this ring, same as the anchors pass above already resolves
      // ring 0's implicit colours (`s.itemStyle?.color ?? seriesColor(i)`).
      const sliceColor = s.itemStyle?.color ?? seriesColor(i);
      const labelColor = getTextColorForBackground(sliceColor, labelTextDark, labelTextLight);

      return { displayLabel, labelColor };
    });

    return {
      type: "pie" as const,
      name: dims[level],
      radius: [fmtPct(ringInnerPct), fmtPct(ringOuterPct)],
      center: ["50%", "48%"],
      data: ringData.map(({ name, value, itemStyle, parentKey }, i) => {
        const base = itemStyle ? { name, value, itemStyle } : { name, value };
        const { displayLabel, labelColor } = fits[i];
        // Metabase's own sentinel for "nothing to show" (option.ts ~L269): a single space.
        //
        // Still REQUIRED even though a dropped label is now a proper `null`, and the null cannot
        // simply be passed through: ECharts reads a formatter returning null/undefined as "no
        // formatter ran" and falls back to its own default label text (label/labelStyle.ts'
        // getLabelText — `if (baseText == null) { baseText = opt.defaultText }`), which for a pie
        // datum is the slice's own name. Returning null would therefore resurrect the very label
        // the fit decision just dropped. A non-null string is returned verbatim (dataFormat.ts'
        // getFormattedLabel), so the sentinel is what actually suppresses the label.
        //
        // A space rather than "": an empty string is indistinguishable from a slice whose
        // dimension value is genuinely blank, and telling those two apart is the whole point of
        // fitSliceLabel's blank-label guard (see the tests that pin it). The cost is that this
        // invisible label still carries a space's width into `labelLayout.hideOverlap` — accepted,
        // and the reason a dropped label is a deliberate sentinel here rather than an accident.
        const withLabel = { ...base, label: { formatter: () => displayLabel ?? " ", color: labelColor } };
        if (!parentFold || !parentValues || parentKey == null) return withLabel;
        const parentFoldKey = parentFold.get(parentKey);
        const parentValue = parentFoldKey != null ? parentValues.get(parentFoldKey) : undefined;
        if (!parentValue) return withLabel; // defensive only — see the note above
        const parentPercent = parseFloat(((value / parentValue) * 100).toFixed(decimals));
        return { ...withLabel, parentPercent };
      }),
      // Series-level label: shared defaults every slice's own `label` (above) falls through to
      // (show, position, fontSize, fontWeight, rotate). Deliberately carries NO `formatter`:
      // every datum above always sets its own, so a series-level one could never run — it existed
      // only as a callable convenience for tests, which then asserted the app's label composition
      // against a function production never executes. Those tests now go through the per-datum
      // formatter that actually renders (see pie.test.ts).
      label: {
        show: showName || showPercent,
        position: "inside" as const,
        fontSize: sliceFontSize,
        fontWeight: 700,
        // Fix 1a: multi-ring labels run along the radius rather than horizontally — a radial
        // label fits a narrow arc a horizontal one can't. ECharts' pie label supports this
        // directly (`rotate: "radial" | "tangential" | number`).
        rotate: labelsPosition === "horizontal" ? 0 : ("radial" as const),
        verticalAlign: "middle" as const,
      },
      labelLayout,
      padAngle: 1,
      itemStyle: ringBorder,
      // Keeps ECharts' own `percent` (ring 0's percent, and the fallback on every other ring's
      // datum that somehow lacks a parentPercent) at the same precision `pct` formats to, so
      // hovering and the in-chart label never disagree.
      percentPrecision: decimals,
    };
  });

  // Every ring's total *should* equal the first ring's (each ring re-partitions the same rows),
  // but the zero/negative filter runs independently per ring (see buildSlices), so a surviving
  // parent can still lose children there — the sign/zero rule is applied per-item on the finer
  // partition, not by re-netting the parent's already-signed total. A screen-reader user has no
  // other way to notice a ring quietly drawing less (or more) than the chart's stated total, so
  // say so explicitly.
  const ringTotals = effectiveRings.map((ring) => ring.reduce((t, s) => t + s.value, 0));
  const divergenceNotes = ringTotals
    .map((t, level) => ({ t, level }))
    .filter(({ t, level }) => level > 0 && Math.abs(t - ringTotals[0]) > 1e-9)
    .map(({ t, level }) => `Note: the ${dims[level]} ring totals ${fmt(t)}, not ${fmt(ringTotals[0])} like the chart's overall total — some rows were dropped by the zero/negative filter at that ring.`);

  // KNOWN GAP, DEFERRED (not solved here): `ariaDescription` gives a screen-reader user ring
  // 0's flat (name, value) list, but says nothing about ring 1+ — beyond two levels a sunburst
  // encodes meaning in colour (family + lightened shade) and position (nested arc under a
  // specific parent wedge) that this text summary cannot recover at all. The standard remedy is
  // a full table/indented-list alternative with each slice's share of its parent, which is a
  // separate unit of work (a real UI, not a string) and is explicitly NOT attempted here — flagged
  // so this gap isn't mistaken for solved by fixes 1/2 above, which only make the VISUAL labels
  // fit and read; they don't add anything for a screen reader.
  const baseAria = ariaDescription({
    kind: "Pie chart", metric: cfg.metric, dimension, format: fmt,
    points: data.map((d) => ({ name: String(d.name), value: d.value })),
  });
  const aria = divergenceNotes.length > 0
    ? { ...baseAria, label: { description: `${baseAria.label.description} ${divergenceNotes.join(" ")}` } }
    : baseAria;

  return {
    ...baseOption(),
    aria,
    tooltip: itemTooltip(fmt, { percent: true }),
    // Ring 0 only (fix 4): every ring's names used to go into one flat legend, but only ring 0's
    // slices have a single, unambiguous colour — an outer ring's "Widgets" can appear once per
    // parent slice, each a different lightened shade, so listing it in the legend would claim a
    // colour it doesn't consistently have.
    //
    // Defaults on (`?? true`, not `||`): a pie's colours are meaningless without the key that
    // says which is which, so an unset value shows the legend; an explicit `false` still turns
    // it off.
    legend: bottomLegend(cfg.showLegend ?? true, data.map((d) => d.name)),
    graphic,
    series,
  };
}
