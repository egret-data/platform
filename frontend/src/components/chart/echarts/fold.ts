export interface FoldPoint {
  name: string;
  value: unknown;
}

/** The label the folded remainder gets, matching Metabase's wording. */
export const OTHER_LABEL = "Other";

/**
 * Folds a long tail into a single "Other" point, keeping the largest `max - 1`.
 *
 * Off unless `max` is given: Metabase's `graph.max_categories_enabled` defaults to false
 * and egret follows, because silently hiding rows behind "Other" is worse than a crowded
 * axis. Order is preserved for the kept points so the chart does not reshuffle when the
 * setting is switched on.
 */
export function foldTail(points: FoldPoint[], max: number | undefined): FoldPoint[] {
  const limit = max == null ? undefined : Math.floor(max);
  if (limit == null || limit < 2 || points.length <= limit) return points;

  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  // Ranked by value, not by magnitude — "top n" the way Superset's series limit and
  // Metabase's grouping both read it. The cost is that one large negative bar can fold
  // away while smaller positive ones stay, which is surprising on a profit-and-loss chart.
  // Ranked by index rather than by name: two points can carry the same name, and a
  // name-keyed lookup would keep both, making the result longer than the limit asked for.
  const keep = new Set(
    points
      .map((_, i) => i)
      .sort((a, b) => num(points[b].value) - num(points[a].value))
      .slice(0, limit - 1),
  );
  const rest = points.reduce((t, p, i) => (keep.has(i) ? t : t + num(p.value)), 0);
  const kept = points.filter((_, i) => keep.has(i));

  // A column can already have a category called "Other". Add the tail to it rather than
  // drawing a second bar with the same name.
  const existing = kept.findIndex((p) => p.name === OTHER_LABEL);
  if (existing >= 0) {
    return kept.map((p, i) => (i === existing ? { ...p, value: num(p.value) + rest } : p));
  }
  return [...kept, { name: OTHER_LABEL, value: rest }];
}
