import type { ResultColumn } from "@/components/chart/ChartTable";
import { buildCartesianOption, type CartesianBuildConfig } from "./cartesian";

/**
 * The two things `ChartRenderer`'s click handler needs to know about a cartesian chart:
 * the bucket a fold produced, if one did, and what real column value sits behind each
 * series' display name.
 *
 * Both used to be read straight off the built option. That stopped being possible once the
 * option became a FUNCTION of the measured container size — you cannot index into a function.
 * Neither value depends on size, so both are derived here instead.
 *
 * The option is still what computes them, so this stays a narrow read of it rather than a second
 * implementation of the folding rules, which would be a copy free to drift. Two consequences for
 * whoever comes next: the read happens with NO size, so should a size-dependent decision ever
 * reach `__foldedCategory` or `series[].__raw` — the size-aware builders land in PRs 02/03/05 —
 * this helper will silently report the no-size answer. And each call builds the option again, so
 * keep the call in the same memo as the option itself.
 */
export interface CartesianMeta {
  /** The name of the bucket the fold produced — always `OTHER_LABEL`, undefined when nothing
   *  folded. It can also be a REAL category of that name, which the fold merges into rather than
   *  drawing a second bar; the drill refuses it either way, since `= 'Other'` would return a
   *  fraction of it. */
  foldedCategory: string | undefined;
  /** The series' real column value at a series index, behind whatever label the legend shows. */
  rawSeriesAt: (i: number | undefined) => string | undefined;
}

export function cartesianMeta(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: CartesianBuildConfig,
): CartesianMeta {
  const option = buildCartesianOption(columns, rows, cfg) as {
    __foldedCategory?: string;
    series?: { __raw?: string }[];
  };
  const series = option.series ?? [];
  return {
    foldedCategory: option.__foldedCategory,
    rawSeriesAt: (i) => (i == null ? undefined : series[i]?.__raw),
  };
}
