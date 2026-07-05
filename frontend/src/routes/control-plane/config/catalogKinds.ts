/**
 * Supported engine-catalog kinds. Shared by `AddCatalogDialog` (EngineDetail.tsx)
 * and `CatalogForm` — kept in its own leaf module (rather than exported from
 * EngineDetail.tsx) so CatalogForm can depend on it without risking a circular
 * import once EngineDetail embeds CatalogForm directly (Task 7).
 */
export const CATALOG_KINDS: { value: string; label: string }[] = [
  { value: "egret-warehouse", label: "Egret warehouse" },
  { value: "postgresql", label: "PostgreSQL" },
  { value: "tpch", label: "TPC-H" },
  { value: "tpcds", label: "TPC-DS" },
  { value: "generic", label: "Generic connector" },
];
