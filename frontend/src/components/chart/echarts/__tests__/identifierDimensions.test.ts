import { describe, it, expect } from "vitest";
import { funnelSteps } from "../options/funnel";
import { buildBoxplotOption } from "../options/boxplot";
import { buildSankeyOption } from "../options/sankey";

/**
 * KI-294: the identifier rule of KI-058 (no thousands separator for an integer `id`, `…_id` or
 * `…key` column) did not reach a chart dimension. With no format at all the value was drawn as
 * stored; with ANY format — even only a Column title or a dataset Display name — it went through
 * the number format and came out `29,989`.
 */
const titled = { orderkey: { column_title: "Order" }, custkey: { column_title: "Customer" } };

describe("identifier dimensions keep no thousands separator (KI-294)", () => {
  it("names a funnel step from an identifier column without a separator", () => {
    const cols = [{ name: "orderkey", type: "bigint" }, { name: "n", type: "double" }];
    const steps = funnelSteps(cols, [[29989, 10], [1200, 5]], { dimension: "orderkey", metric: "n", columnSettings: titled });
    expect(steps.map((s) => s.name)).toEqual(["29989", "1200"]);
  });

  it("labels a box plot category from an identifier column without a separator", () => {
    const cols = [{ name: "orderkey", type: "bigint" }, { name: "v", type: "double" }];
    const opt = buildBoxplotOption(cols, [[29989, 1], [29989, 2], [1200, 3]], {
      category: "orderkey", value: "v", columnSettings: titled,
    }) as any;
    expect(opt.xAxis.data.map((c: unknown, i: number) => opt.xAxis.axisLabel.formatter(c, i))).toEqual(["1200", "29989"]);
  });

  it("labels a Sankey node from an identifier column without a separator", () => {
    const cols = [{ name: "custkey", type: "bigint" }, { name: "orderkey", type: "bigint" }, { name: "v", type: "double" }];
    const opt = buildSankeyOption(cols, [[12345, 29989, 10]], {
      source: "custkey", target: "orderkey", metric: "v", columnSettings: titled,
    }) as any;
    const s = opt.series[0];
    expect(s.data.map((n: { name: string }) => s.label.formatter({ name: n.name }))).toEqual(["12345", "29989"]);
  });

  it("still groups a measure-like integer dimension when it has a format", () => {
    const cols = [{ name: "year_total", type: "bigint" }, { name: "n", type: "double" }];
    const steps = funnelSteps(cols, [[29989, 10]], { dimension: "year_total", metric: "n", columnSettings: { year_total: { column_title: "Total" } } });
    expect(steps[0].name).toBe("29,989");
  });

  it("honours a separator style chosen for the identifier", () => {
    const cols = [{ name: "orderkey", type: "bigint" }, { name: "n", type: "double" }];
    const steps = funnelSteps(cols, [[29989, 10]], { dimension: "orderkey", metric: "n", columnSettings: { orderkey: { number_separators: ".," } } });
    expect(steps[0].name).toBe("29,989");
  });
});
