import { describe, it, expect } from "vitest";
import { resolveClick } from "../click-behavior";
import type { DashboardParameter } from "@/api/dashboards";
import type { DrillContext } from "@/components/chart/drill";

const ctx: DrillContext = { dims: [{ col: "region", value: "EU" }, { col: "status", value: "paid" }] };
const params: DashboardParameter[] = [
  { id: "pr", name: "Region", slug: "region", type: "text", operator: "is" },
  { id: "ps", name: "Status", slug: "stat", type: "category", operator: "is" },
];

describe("resolveClick", () => {
  it("menu when no click behaviour", () => {
    expect(resolveClick(undefined, ctx, params, {})).toEqual({ kind: "menu", ctx });
    expect(resolveClick({ type: "menu" }, ctx, params, {})).toEqual({ kind: "menu", ctx });
  });

  it("crossfilter resolves each mapped column to its parameter slug", () => {
    const cb = { type: "crossfilter" as const, parameterMapping: { pr: "region", ps: "status" } };
    expect(resolveClick(cb, ctx, params, {}).kind).toBe("crossfilter");
    const r = resolveClick(cb, ctx, params, {}) as { kind: "crossfilter"; sets: { slug: string; value: unknown }[] };
    expect(r.sets).toEqual([{ slug: "region", value: "EU" }, { slug: "stat", value: "paid" }]);
  });

  it("crossfilter skips a parameter whose mapped column was not clicked", () => {
    const cb = { type: "crossfilter" as const, parameterMapping: { pr: "missing" } };
    const r = resolveClick(cb, ctx, params, {}) as { kind: "crossfilter"; sets: unknown[] };
    expect(r.sets).toEqual([]);
  });

  it("url interpolates {{column}}, {{value}}, and {{slug}}, then guards http(s)", () => {
    const cb = { type: "url" as const, template: "https://x.com/{{region}}?v={{value}}&from={{stat}}" };
    const r = resolveClick(cb, ctx, params, { stat: "open" }) as { kind: "url"; href: string | null };
    expect(r.href).toBe("https://x.com/EU?v=EU&from=open");
  });

  it("url rejects a non-http(s) result (XSS guard)", () => {
    const cb = { type: "url" as const, template: "javascript:alert({{region}})" };
    const r = resolveClick(cb, ctx, params, {}) as { kind: "url"; href: string | null };
    expect(r.href).toBeNull();
  });
});
