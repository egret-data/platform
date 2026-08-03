import { describe, it, expect } from "vitest";
import { pivotDescriptor } from "../pivot";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" }, orderby: [] };
const d = pivotDescriptor();

describe("pivotDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(d.requiredFields({ rows: [], cols: [], metrics: [] })).toEqual(["a row dimension", "a value"]);
  });

  it("builds a spec with rows then cols as dimensions and each group as a rollup", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: ["category"], metrics: ["total_revenue"] }, SHARED);
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.rollup_groups).toEqual([["region"], ["category"]]);
  });

  it("omits an empty group from the rollups", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"] }, SHARED);
    expect(spec.rollup_groups).toEqual([["region"]]);
  });

  it("keeps interactive state out of the spec so collapsing never re-queries", () => {
    const a = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"] }, SHARED);
    const b = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"], collapsedRows: ["region:North"], sort: { region: "desc" } }, SHARED);
    expect(b).toEqual(a);
  });

  it("defaults the three totals toggles to on", () => {
    expect(d.defaultConfig.showRowTotals).toBe(true);
    expect(d.defaultConfig.showColTotals).toBe(true);
    expect(d.defaultConfig.condense).toBe(true);
  });

  it("reads a v1 params object from its loose top-level keys", () => {
    const cfg = d.fromParams({
      spec: EMPTY_SPEC, rows: ["region"], cols: [], metrics: ["m"],
      showRowTotals: false, collapsedRows: ["region:North"], pivotSort: { region: "desc" },
    });
    expect(cfg.rows).toEqual(["region"]);
    expect(cfg.showRowTotals).toBe(false);
    expect(cfg.collapsedRows).toEqual(["region:North"]);
    expect(cfg.sort).toEqual({ region: "desc" });
  });

  it("round-trips config → settings → config", () => {
    const cfg = {
      rows: ["region"], cols: ["category"], metrics: ["m"],
      showRowTotals: false, showColTotals: true, condense: true,
      collapsedRows: ["x"], sort: { region: "asc" as const },
    };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("writes only namespaced keys", () => {
    const s = d.toSettings({ rows: ["region"], cols: [], metrics: ["m"] });
    expect(Object.keys(s).every((k) => k.startsWith("pivot."))).toBe(true);
  });
});
