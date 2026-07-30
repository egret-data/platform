import { describe, it, expect } from "vitest";
import { tableDescriptor, objectDescriptor } from "../table";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" } };

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
});
