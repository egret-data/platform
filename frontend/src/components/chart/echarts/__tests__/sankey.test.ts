import { describe, it, expect } from "vitest";
import { buildSankeyOption } from "../options/sankey";

const cols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
const rows = [["A", "X", 10], ["A", "Y", 5], ["B", "X", 3]];

describe("buildSankeyOption", () => {
  it("builds unique nodes and weighted links", () => {
    const opt = buildSankeyOption(cols, rows, { source: "s", target: "t", metric: "v" }) as any;
    expect(opt.series[0].type).toBe("sankey");
    expect(opt.series[0].data.map((n: any) => n.name).sort()).toEqual(["A", "B", "X", "Y"]);
    expect(opt.series[0].links).toEqual([
      { source: "A", target: "X", value: 10 },
      { source: "A", target: "Y", value: 5 },
      { source: "B", target: "X", value: 3 },
    ]);
  });
  it("drops non-positive / non-finite links", () => {
    const opt = buildSankeyOption(cols, [["A", "X", 10], ["A", "Y", 0], ["B", "X", -2]], { source: "s", target: "t", metric: "v" }) as any;
    expect(opt.series[0].links.map((l: any) => l.source)).toEqual(["A"]);
  });
  it("empty when source / target / metric missing", () => {
    expect((buildSankeyOption(cols, rows, {}) as any).series[0].links).toEqual([]);
  });
});
