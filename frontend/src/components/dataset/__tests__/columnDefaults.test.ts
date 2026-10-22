import { describe, expect, test } from "vitest";
import { datasetColumnDefaults, withDatasetDefaults } from "@/components/dataset/columnDefaults";
import { filterableColumns, pickableColumns } from "@/components/dataset/pickableColumns";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

const col = (column_name: string, extra: Partial<DatasetColumn> = {}): DatasetColumn => ({
  column_name, data_type: "varchar", type_generic: "string", is_temporal: false, ordinal: 0,
  expression: null, verbose_name: "", description: "", is_calculated: false, groupby: true,
  filterable: true, format: "", semantic_type: "", ...extra,
});
const metric = (metric_name: string, verbose_name = ""): DatasetMetric => ({
  id: metric_name, metric_name, expression: "count(*)", data_type: "bigint", type_generic: "numeric",
  verbose_name, description: "", ordinal: 0, format: "", certified: false, certified_by: "", certification_details: "",
});

describe("datasetColumnDefaults (KI-103)", () => {
  test("a Display name becomes the default column title, for columns and metrics", () => {
    const d = datasetColumnDefaults([col("order_size", { verbose_name: "Order size" }), col("raw")], [metric("n", "Orders")]);
    expect(d.order_size).toEqual({ column_title: "Order size" });
    expect(d.raw).toBeUndefined();
    expect(d.n).toEqual({ column_title: "Orders" });
  });

  test("the chart's own settings win, but keep the display name unless they set a title", () => {
    const defaults = { a: { column_title: "Alpha", decimals: 1 }, b: { column_title: "Beta" } };
    const out = withDatasetDefaults(defaults, { a: { decimals: 3 }, b: { column_title: "Mine" } });
    expect(out.a).toEqual({ decimals: 3, column_title: "Alpha" });
    expect(out.b).toEqual({ column_title: "Mine" });
  });
});

describe("pickableColumns (KI-096)", () => {
  const cols = [col("region"), col("orderkey", { groupby: false }), col("legacy", { groupby: false })];
  test("offers Dimension-on columns and those the chart already uses", () => {
    const names = pickableColumns(cols, { recordsView: false, inUse: [{ xAxis: "legacy" }] }).map((c) => c.column_name);
    expect(names).toEqual(["region", "legacy"]);
  });
  test("a records view lists every column", () => {
    expect(pickableColumns(cols, { recordsView: true, inUse: [] })).toHaveLength(3);
  });
  test("a dashboard filter maps only onto Filter-on columns, keeping the current mapping", () => {
    const off = new Set(["orderkey", "legacy"]);
    expect(filterableColumns(["region", "orderkey", "legacy"], off, "legacy")).toEqual(["region", "legacy"]);
    expect(filterableColumns(["region", "orderkey"], undefined)).toEqual(["region", "orderkey"]);
  });
});
