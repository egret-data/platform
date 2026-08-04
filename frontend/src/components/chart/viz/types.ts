import type { FC, ReactNode } from "react";
import type { AnyChartParams, ChartFilter, ChartOrderBy, QuerySpec, VizSettings, VizType } from "@/api/charts";
import type { ChartTypeMeta } from "@/components/chart/chartTypes";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

/** Query state every viz shares; descriptors fold it into the spec they build. */
export interface SharedQueryState {
  filters: ChartFilter[];
  rowLimit: number;
  temporalUnits: Record<string, string>;
  /**
   * The sort the CHART'S AUTHOR asked for, in order — sort is a query concern, so it sits beside
   * `filters` and `rowLimit` rather than in any viz's settings, which is also how Metabase models
   * it (a notebook step, not a visualization option).
   *
   * Empty is the normal case and means "no explicit sort": a descriptor that derives its own
   * ascending default (cartesian, smartscalar) keeps deriving it, and one that asks the engine for
   * nothing keeps asking for nothing. See `add-implicit-breakout-order-by`
   * (metabase/query_processor/middleware/add_implicit_clauses.clj) for the precedence rule the
   * cartesian descriptor ports: the implicit ascending clause is added for each breakout *unless*
   * that field is already explicitly referenced here.
   */
  orderby: ChartOrderBy[];
}

/** What a control gets: its config, a patcher, the dataset, and the last result. */
export interface VizControlProps<Cfg> {
  config: Cfg;
  onChange: (patch: Partial<Cfg>) => void;
  /** Dataset columns and metrics — what the chart *can* be configured with. */
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
  /**
   * Columns and rows of the last result. Controls that list series (per-series colours,
   * labels) derive their own list from these — the shell must not compute anything
   * viz-specific on their behalf.
   */
  resultColumns: { name: string; type: string }[];
  rows: unknown[][];
  /**
   * The temporal unit each dimension of THAT result was grouped by, keyed by column name — the
   * `spec.temporal_units` the query on screen actually ran with, which is the same map
   * `ChartRenderer` hands the builders.
   *
   * A control needs it for the same reason a builder does: under an EXTRACTION unit
   * (`day_of_week`, `hour_of_day`, …) the value in the column is not the value the chart keys on.
   * The pie aggregates its ring on `temporalLabel(unit, raw)` — "Wed", not the `3` in the row — so
   * a slice list built by stringifying the column offered rows whose renames, hides and pinned
   * colours were all written under a key no slice carries. Empty is the normal case and means
   * "no bucketing", which is what every non-temporal chart passes.
   *
   * FROZEN WITH THE RESULT, not the live editor state: `rows` above came back from a query that
   * ran with a particular set of units, and pairing them with units the user has since changed
   * describes a chart nobody is looking at. Both call sites read them off the same params the
   * preview renders from.
   */
  temporalUnits: Record<string, string>;
  /** Renders the shared "Date grouping" row for a temporal dimension, or null. */
  unitRow: (col?: string) => ReactNode;
}

export interface VizControl<Cfg> {
  id: string;
  section: "data" | "display";
  /** Heading this control sits under; ungrouped controls render before any group. */
  group?: string;
  /** Hidden behind the group's "More options" disclosure. */
  advanced?: boolean;
  render: FC<VizControlProps<Cfg>>;
}

/**
 * Everything the studio shell needs to know about one visualization. The shell never
 * reads inside `Cfg`; only this descriptor does.
 */
export interface VizDescriptor<Cfg = unknown> {
  key: VizType;
  meta: ChartTypeMeta;
  /** Settings namespace, e.g. "cartesian." — this descriptor owns only these keys. */
  prefix: string;
  defaultConfig: Cfg;

  fromParams(params: AnyChartParams): Cfg;
  toSettings(cfg: Cfg): VizSettings;
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;
  requiredFields(cfg: Cfg): string[];

  controls: VizControl<Cfg>[];
}
