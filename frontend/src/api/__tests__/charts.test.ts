import { describe, it, expect } from "vitest";
import type { ChartParams } from "../charts";

describe("ChartParams", () => {
  it("carries spec + pivot layout", () => {
    const p: ChartParams = {
      spec: { metrics: ["revenue"], dimensions: ["region", "seg"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [["region"], ["seg"]] },
      rows: ["region"], cols: ["seg"], metrics: ["revenue"], showRowTotals: true, showColTotals: true, condense: true,
    };
    expect(p.spec.rollup_groups).toEqual([["region"], ["seg"]]);
    expect(p.rows).toEqual(["region"]);
  });
});
