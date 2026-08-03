import { describe, it, expect } from "vitest";
import { tableDescriptor, objectDescriptor } from "../table";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" }, orderby: [] };

describe("tableDescriptor", () => {
  const d = tableDescriptor();

  it("needs no metric — a bare table lists raw rows", () => {
    expect(d.requiredFields({ columns: ["region"], metrics: [] })).toEqual([]);
  });

  // Reporting "ready" for an empty config made the runner fire `columns: [], metrics: []`, which
  // the backend answers 400 — two console errors on a page the user had not yet touched.
  it("asks for a field when nothing is picked instead of running an empty query", () => {
    expect(d.requiredFields({ columns: [], metrics: [] })).toEqual(["at least one column or metric"]);
  });

  it("groups by the chosen columns when metrics are picked", () => {
    const spec = d.buildSpec({ columns: ["region"], metrics: ["total_revenue"] }, SHARED);
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.columns).toEqual([]);
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("selects raw columns when no metric is picked", () => {
    const spec = d.buildSpec({ columns: ["region", "category"], metrics: [] }, SHARED);
    expect(spec.dimensions).toEqual([]);
    expect(spec.columns).toEqual(["region", "category"]);
  });

  it("reads a v1 table chart back out of its spec", () => {
    const cfg = d.fromParams({ spec: { ...EMPTY_SPEC, metrics: ["m"], dimensions: ["region"], columns: [] } });
    expect(cfg).toEqual({ columns: ["region"], metrics: ["m"] });
  });

  it("round-trips config → settings → config", () => {
    const cfg = { columns: ["region"], metrics: ["m"] };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  // A v2 row whose `settings` never got the `table.*` slice — written by a seeding harness, not
  // by the editor — opened on "Add: at least one column or metric" with its ten columns sitting
  // right there in the spec. v1 recovered them; v2 must too.
  it("recovers the columns from the spec when the v2 settings have no table slice", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, columns: ["reviewer", "rating", "product"] },
      settings: {},
    });
    expect(cfg).toEqual({ columns: ["reviewer", "rating", "product"], metrics: [] });
  });

  it("recovers an aggregated table's dimensions and metrics from the spec", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, dimensions: ["region"], columns: ["category"], metrics: ["total_revenue"] },
      settings: {},
    });
    expect(cfg).toEqual({ columns: ["region", "category"], metrics: ["total_revenue"] });
  });

  // The fixture above would pass on the broken code if the spec and the settings agreed, so the
  // stored slice must WIN over a spec that says something different.
  it("prefers the stored table slice over the spec when both are present", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, dimensions: ["from_spec_dim"], columns: ["from_spec_col"], metrics: ["from_spec_m"] },
      settings: { "table.columns": ["stored_col"], "table.metrics": ["stored_m"] },
    });
    expect(cfg).toEqual({ columns: ["stored_col"], metrics: ["stored_m"] });
  });

  // Deliberately emptied is not the same as never written: a metrics-only table stores
  // `table.columns: []`, and re-deriving columns from the spec would put them back.
  it("keeps a stored-but-empty column list empty", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, columns: ["from_spec_col"], metrics: ["total_revenue"] },
      settings: { "table.columns": [], "table.metrics": ["total_revenue"] },
    });
    expect(cfg).toEqual({ columns: [], metrics: ["total_revenue"] });
  });
});

describe("objectDescriptor", () => {
  const d = objectDescriptor();

  it("needs at least one column", () => {
    expect(d.requiredFields({ columns: [] })).toEqual(["at least one column"]);
    expect(d.requiredFields({ columns: ["region"] })).toEqual([]);
  });

  it("selects its columns raw", () => {
    const spec = d.buildSpec({ columns: ["region"] }, SHARED);
    expect(spec.columns).toEqual(["region"]);
    expect(spec.metrics).toEqual([]);
  });

  it("reads a v1 object chart from its spec columns", () => {
    expect(d.fromParams({ spec: { ...EMPTY_SPEC, columns: ["a", "b"] } })).toEqual({ columns: ["a", "b"] });
  });

  it("falls back to the table columns so switching table→object keeps the selection", () => {
    const cfg = d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: { "table.columns": ["region"] } });
    expect(cfg).toEqual({ columns: ["region"] });
  });

  it("prefers its own columns over the table fallback", () => {
    const cfg = d.fromParams({
      version: 2, spec: EMPTY_SPEC,
      settings: { "object.columns": ["a"], "table.columns": ["b"] },
    });
    expect(cfg).toEqual({ columns: ["a"] });
  });

  it("recovers the columns from the spec when the v2 settings have neither slice", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, columns: ["id", "plan", "seats"] },
      settings: {},
    });
    expect(cfg).toEqual({ columns: ["id", "plan", "seats"] });
  });

  it("prefers a stored slice over the spec", () => {
    const own = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, columns: ["from_spec"] },
      settings: { "object.columns": ["stored"] },
    });
    expect(own).toEqual({ columns: ["stored"] });

    const inherited = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, columns: ["from_spec"] },
      settings: { "table.columns": ["from_table"] },
    });
    expect(inherited).toEqual({ columns: ["from_table"] });
  });
});
