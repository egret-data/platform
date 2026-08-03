/**
 * Chart params for the "ZZ Chart Parity" harness, built the way the app builds them.
 *
 * The harness has lied about chart params four times, and every time the same way: it wrote
 * `params` by hand while the app writes them through its viz descriptors. Wrong settings keys
 * (`pie.dimension` for `pie.dimensions`), an empty `rollup_groups` on the pivot, `settings: {}`
 * on the table charts, `temporal_units: {}` everywhere. Each cost a PR real work before anyone
 * noticed the harness — not the app — was wrong.
 *
 * So nothing here knows what a pie's settings key is called. `harnessParams` is a transcription
 * of the studio's `paramsForSave()` (frontend/src/routes/explore/ChartExplore.tsx) and delegates
 * every decision to `getDescriptor(viz)`. Adding a chart type to the harness requires no change
 * to this file, and getting a settings key wrong is not expressible.
 *
 * Run under `vite-node` from `frontend/` (the `@` alias and the React plugin live in its Vite
 * config); type-checked by `frontend`'s `tsc --noEmit`, tested by its `vitest run`.
 */
import { getDescriptor } from "@/components/chart/viz/registry";
import { mergeVizSettings } from "@/components/chart/viz/renderConfig";
import { withDefaultTemporalUnits } from "@/components/chart/temporal-units";
import type { ChartFilter, ChartOrderBy, ChartParamsV2, VizSettings, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";

/**
 * One harness chart, described the way the STUDIO holds one: a viz type plus that viz's own
 * config object (the shape its descriptor's `defaultConfig` describes). Never settings, and
 * never a spec — those are outputs.
 */
export interface HarnessChart {
  viz: VizType;
  /** The descriptor's config. Opaque here: only the descriptor ever looks inside it. */
  config: unknown;
  /**
   * The dataset's columns, or at least their names and whether each is a date. Needed only
   * for the monthly-grouping default below.
   */
  columns: { column_name: string; is_temporal?: boolean }[];
  /** Defaults below mirror ChartExplore's initial state for a new chart. */
  filters?: ChartFilter[];
  rowLimit?: number;
  /**
   * The sort the chart's author asked for, if any. Shared query state, so it lives here beside
   * `filters` and `rowLimit` rather than in `config` — and, like them, defaults to the studio's
   * initial state for a new chart, which is none. A descriptor that derives its own order
   * (cartesian, smartscalar) still does; see `SharedQueryState`.
   */
  orderby?: ChartOrderBy[];
  /** Explicit temporal units. A date dimension not named here defaults to "month". */
  temporalUnits?: Record<string, string>;
  /** Settings the chart already carried, e.g. keys no descriptor in this build reads. */
  settings?: VizSettings;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
}

/**
 * The one rule the descriptors do not carry, applied here rather than duplicated: the studio
 * defaults a date dimension to monthly grouping, and `buildSpec` only ever prunes the units it
 * is handed. `withDefaultTemporalUnits` is that rule, and `ChartExplore`'s `useEffect` now calls
 * the same function — so this is shared code, not a second copy that can drift.
 *
 * The studio reaches the fixed point across two renders (effect sets state, next render saves);
 * one pass is enough here because no descriptor's `dimensions` depend on the units.
 */
function specTemporalUnits(chart: HarnessChart, dims: string[]): Record<string, string> {
  return withDefaultTemporalUnits(chart.temporalUnits ?? {}, dims, chart.columns);
}

/**
 * Byte-identical to the studio's `paramsForSave()`. Kept as one expression against the same
 * three calls (`buildSpec`, `mergeVizSettings`, and the two shell keys) so a diff against
 * ChartExplore is a one-screen read.
 */
export function harnessParams(chart: HarnessChart): ChartParamsV2 {
  const descriptor = getDescriptor(chart.viz);
  if (!descriptor) throw new Error(`no descriptor for viz ${chart.viz}`);
  const config = chart.config as never;

  const shared = {
    filters: chart.filters ?? [],
    rowLimit: chart.rowLimit ?? 1000,
    temporalUnits: chart.temporalUnits ?? {},
    orderby: chart.orderby ?? [],
  };
  // Build once to learn the dimensions, apply the monthly default, build again with it.
  const dims = descriptor.buildSpec(config, shared).dimensions;
  const spec = descriptor.buildSpec(config, {
    ...shared,
    temporalUnits: specTemporalUnits(chart, dims),
  });

  const settings: VizSettings = {
    // mergeVizSettings, not a bare spread — same reason the studio uses it: a setting set back
    // to "Auto" must LEAVE the stored settings, and a spread can only add or overwrite.
    ...mergeVizSettings(chart.settings ?? {}, descriptor, config),
    column_settings: chart.columnSettings ?? {},
    conditional_formatting: chart.conditionalFormatting ?? [],
  };

  // Trap 4 as an invariant rather than a comment. A chart saved with no key in its own viz
  // namespace looks fine in the database and opens on "Set up your Table", because the editor
  // falls back to recovering the config from the spec. `column_settings` and
  // `conditional_formatting` are always written, so checking `settings` is non-empty would not
  // catch it — the namespace is what has to be non-empty.
  const own = Object.keys(settings).filter((k) => k.startsWith(descriptor.prefix));
  if (own.length === 0) {
    throw new Error(
      `${chart.viz}: config produced no ${descriptor.prefix}* settings — the chart would open ` +
      `on its setup prompt. Check the config against the descriptor's defaultConfig.`,
    );
  }

  return { version: 2, spec, settings };
}
