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
  if (max == null || max < 2 || points.length <= max) return points;
  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  const ranked = [...points].sort((a, b) => num(b.value) - num(a.value));
  const keep = new Set(ranked.slice(0, max - 1).map((p) => p.name));
  const kept = points.filter((p) => keep.has(p.name));
  const rest = points.filter((p) => !keep.has(p.name)).reduce((t, p) => t + num(p.value), 0);
  return [...kept, { name: OTHER_LABEL, value: rest }];
}
