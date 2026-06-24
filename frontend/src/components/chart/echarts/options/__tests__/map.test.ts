import { describe, it, expect } from "vitest";
import { buildMapOption } from "../map";

const cols = [{ name: "state", type: "text" }, { name: "revenue", type: "double" }];
const rows = [["nh", 100], ["me", 250], ["bad", Number.NaN]];

describe("buildMapOption", () => {
  it("normalizes region keys, drops NaN, sets visualMap range and map name", () => {
    const opt = buildMapOption(cols, rows, {
      echartsName: "us_states", region: "state", metric: "revenue",
      normalize: (v) => String(v ?? "").trim().toUpperCase(),
    }) as {
      series: { type: string; map: string; data: { name: string; value: number }[] }[];
      visualMap: { min: number; max: number };
    };
    expect(opt.series[0].type).toBe("map");
    expect(opt.series[0].map).toBe("us_states");
    const data = opt.series[0].data;
    expect(data.map((d) => d.name)).toEqual(["NH", "ME"]);
    expect(data.find((d) => d.name === "ME")?.value).toBe(250);
    expect(opt.visualMap.min).toBe(100);
    expect(opt.visualMap.max).toBe(250);
  });

  it("returns an empty-but-valid option when there is no data", () => {
    const opt = buildMapOption(cols, [], { echartsName: "world", region: "state", metric: "revenue", normalize: (v) => String(v ?? "") }) as { series: { data: unknown[] }[] };
    expect(opt.series[0].data).toEqual([]);
  });
});
