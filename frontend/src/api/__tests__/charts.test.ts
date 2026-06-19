import { describe, it, expect } from "vitest";
import type { ChartParams } from "../charts";

describe("ChartParams", () => {
  it("carries spec + pivot layout", () => {
    const p: ChartParams = {
      spec: { metrics: ["revenue"], dimensions: ["region", "seg"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [["region"], ["seg"]] },
      rows: ["region"], cols: ["seg"], metrics: ["revenue"], showRowTotals: true, showColTotals: true, condense: true,
      columnSettings: { revenue: { number_style: "currency", currency: "USD" } },
      conditionalFormatting: [{ columns: ["revenue"], type: "single", operator: ">", value: 100, color: "#ff0000" }],
    };
    expect(p.spec.rollup_groups).toEqual([["region"], ["seg"]]);
    expect(p.rows).toEqual(["region"]);
    expect(p.columnSettings?.revenue.number_style).toBe("currency");
    expect(p.conditionalFormatting?.[0].type).toBe("single");
  });
});
