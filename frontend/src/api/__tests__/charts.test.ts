import { describe, it, expect } from "vitest";
import type { ChartParams } from "../charts";

describe("ChartParams", () => {
  it("carries spec + pivot layout", () => {
    const p: ChartParams = {
      spec: { metrics: ["revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup: true },
      rows: ["region"], cols: ["segment"], metric: "revenue",
    };
    expect(p.spec.rollup).toBe(true);
    expect(p.rows).toEqual(["region"]);
  });
});
