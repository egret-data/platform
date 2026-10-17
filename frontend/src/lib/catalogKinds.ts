/**
 * The display name of each registry catalog kind (`registry.catalogs.kind`).
 *
 * A leaf module with no imports, so a page can name a kind without pulling in
 * the catalog create form's pickers (and the API modules behind them). The
 * create page's `KIND_OPTIONS` (`routes/control-plane/catalogs/kinds/pickers.tsx`)
 * takes its labels from here.
 */
export const CATALOG_KIND_LABELS = {
  iceberg: "Iceberg",
  hive: "Hive",
  delta_lake: "Delta Lake",
  hudi: "Hudi",
  jdbc: "JDBC",
  elasticsearch: "Elasticsearch",
  tpch: "TPC-H",
  tpcds: "TPC-DS",
  generic: "Generic",
} as const satisfies Record<string, string>;

/** The display name of `kind`, or the kind itself when egret has no name for it. */
export function catalogKindLabel(kind: string): string {
  return (CATALOG_KIND_LABELS as Record<string, string>)[kind] ?? kind;
}
