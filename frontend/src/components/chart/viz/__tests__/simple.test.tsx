import { describe, it, expect } from "vitest";
import { SIMPLE_DESCRIPTORS } from "../simple";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" }, orderby: [] };
const byKey = Object.fromEntries(SIMPLE_DESCRIPTORS.map((d) => [d.key, d]));

describe("the nine simple descriptors", () => {
  it("covers exactly the viz types PR3 migrates", () => {
    expect(SIMPLE_DESCRIPTORS.map((d) => d.key).sort()).toEqual(
      ["boxplot", "funnel", "gauge", "pie", "progress", "sankey", "scalar", "smartscalar", "waterfall"],
    );
  });

  it("keeps pie's required labels", () => {
    expect(byKey.pie.requiredFields({})).toEqual(["a dimension", "a metric"]);
  });

  it("keeps scalar's single-row spec", () => {
    const spec = byKey.scalar.buildSpec({ metric: "m" }, SHARED);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(1);
  });

  it("keeps smartscalar's ordering by its dimension", () => {
    const spec = byKey.smartscalar.buildSpec({ dimension: "order_date", metric: "m" }, SHARED);
    expect(spec.orderby).toEqual([{ field: "order_date", desc: false }]);
  });

  it("keeps boxplot on raw columns", () => {
    const spec = byKey.boxplot.buildSpec({ category: "region", value: "revenue" }, SHARED);
    expect(spec.columns).toEqual(["region", "revenue"]);
    expect(spec.metrics).toEqual([]);
    expect(spec.dimensions).toEqual([]);
  });

  it("keeps sankey's two dimensions in source→target order", () => {
    const spec = byKey.sankey.buildSpec({ source: "a", target: "b", metric: "m" }, SHARED);
    expect(spec.dimensions).toEqual(["a", "b"]);
  });

  it("prunes temporal units to the grouped dimension", () => {
    expect(byKey.pie.buildSpec({ dimensions: ["order_date"], metric: "m" }, SHARED).temporal_units)
      .toEqual({ order_date: "month" });
    expect(byKey.pie.buildSpec({ dimensions: ["region"], metric: "m" }, SHARED).temporal_units).toEqual({});
  });

  // `dimensions` comes from stored settings, not typed input — a pre-migration row (or
  // anything else that slipped past a server-side check) can hand back a bare string, a
  // number, or an object instead of an array. Every one of those must degrade to "no
  // dimensions" rather than reaching `strs()`, which assumes an array and throws on `.map`.
  it("degrades a malformed stored `dimensions` value to an empty list instead of crashing", () => {
    for (const bad of ["region", 5, { region: true }]) {
      const spec = byKey.pie.buildSpec({ dimensions: bad, metric: "m" }, SHARED);
      expect(spec.dimensions).toEqual([]);
    }
  });

  // Migration 0099 flattened the legacy `pie.dimension` (singular) into settings under
  // that exact key; migration 0103 (task 3, same PR) renames it to the plural `pie.dimensions`
  // array. Until 0103 runs, `fromParams` reads the old key back as `cfg.dimension`, which
  // nothing downstream reads any more — so a pre-0103 pie compiles a spec with no
  // dimensions and renders blank (rows still come back, so the empty-state guard never
  // fires). This pins that gap; it is expected to flip to a non-empty `["region"]` once
  // task 3's migration lands in this same PR, at which point this assertion inverts.
  it("a pre-0103 pie has no dimensions until the migration lifts its key", () => {
    const cfg = byKey.pie.fromParams({
      version: 2, spec: EMPTY_SPEC, settings: { "pie.dimension": "region", "pie.metric": "rev" },
    });
    const spec = byKey.pie.buildSpec(cfg, SHARED);
    expect(spec.dimensions).toEqual([]);
  });

  it("round-trips every descriptor's config through settings", () => {
    const sample: Record<string, Record<string, unknown>> = {
      pie: { dimensions: ["region"], metric: "m" },
      scalar: { metric: "m" },
      progress: { metric: "m", goal: 10 },
      gauge: {
        metric: "m",
        segments: [
          { min: 0, max: 50, color: "red", label: "Low" },
          { min: 50, max: 100, color: "green" },
        ],
      },
      funnel: { dimension: "region", metric: "m" },
      waterfall: { xAxis: "region", metric: "m", showTotal: true },
      smartscalar: { dimension: "order_date", metric: "m" },
      boxplot: { category: "region", value: "revenue" },
      sankey: { source: "a", target: "b", metric: "m" },
    };
    for (const d of SIMPLE_DESCRIPTORS) {
      const cfg = sample[d.key];
      expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
    }
  });

  it("reads every descriptor's v1 nested key", () => {
    expect(byKey.waterfall.fromParams({ spec: EMPTY_SPEC, waterfall: { xAxis: "region", metric: "m" } }))
      .toEqual({ xAxis: "region", metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: EMPTY_SPEC, boxplot: { category: "c", value: "v" } }))
      .toEqual({ category: "c", value: "v" });
  });

  it("reads a pre-nested-key v1 chart from its spec", () => {
    expect(byKey.scalar.fromParams({ spec: { ...EMPTY_SPEC, metrics: ["m"] } })).toEqual({ metric: "m" });
    expect(byKey.boxplot.fromParams({ spec: { ...EMPTY_SPEC, columns: ["c", "v"] } }))
      .toEqual({ category: "c", value: "v" });
  });

  // The pie's display settings are edited by PieDisplaySection, not by a declared field,
  // so a descriptor that only round-tripped its `fields` would drop them — and a saved
  // donut would come back as a plain pie.
  it("round-trips display settings that no field declares", () => {
    const cfg = { dimension: "category", metric: "m", donut: true, showLegend: true, sliceColors: { A: "#fff" } };
    const back = byKey.pie.fromParams({ version: 2, spec: EMPTY_SPEC, settings: byKey.pie.toSettings(cfg) });
    expect(back).toEqual(cfg);
  });

  // Multi-dimension shape (task 1): `dimensions` and the slice-curation fields (e.g.
  // `sliceNames`) are stored keys under the pie prefix like any other, so they need no
  // codec change to round-trip — `fromParams` reads every key under the prefix, not
  // only the ones `fields` declares.
  it("round-trips the multi-dimension shape, including slice-curation fields", () => {
    const cfg = { dimensions: ["region", "category"], metric: "m", sliceNames: { A: "Alpha" } };
    const back = byKey.pie.fromParams({ version: 2, spec: EMPTY_SPEC, settings: byKey.pie.toSettings(cfg) });
    expect(back).toEqual(cfg);
  });

  // Adding a value to an enum owes no migration, but it does owe proof that the codec carries
  // it: `pie.percentVisibility` is a plain key under the prefix, so "legend" round-trips for
  // the same reason every other display setting does — and an unknown stored value survives
  // the trip untouched too, because the codec's job is storage, not validation (the RENDERER
  // is where an unrecognised value degrades to off — see resolvePercentVisibility).
  it("round-trips the new legend percent-visibility, and does not eat an unknown one", () => {
    for (const percentVisibility of ["legend", "both", "everywhere"]) {
      const cfg = { dimensions: ["region", "category"], metric: "m", percentVisibility };
      const back = byKey.pie.fromParams({ version: 2, spec: EMPTY_SPEC, settings: byKey.pie.toSettings(cfg) });
      expect(back).toEqual(cfg);
    }
  });

  it("gives a display control to the descriptors that have display settings, and none to the rest", () => {
    // Each of these owns settings that are presentation rather than query shape: pie (slice
    // colours), gauge (ranges), scalar (captions + conditional colours), smartscalar (colour
    // direction + compact), progress (bar colour + goal-from-a-metric), funnel (step order and
    // visibility), waterfall and boxplot (their whole Display + Axes tabs). A descriptor that is
    // only field pickers must stay purely `data` — that is what keeps the Data tab asking one
    // question.
    for (const key of ["pie", "gauge", "scalar", "smartscalar", "progress", "funnel", "waterfall", "boxplot", "sankey"]) {
      expect(byKey[key].controls.some((c) => c.section === "display")).toBe(true);
    }
  });

  // The waterfall and the box plot declare an Axes group, which is the shell's cue to give it its
  // own tab. The SANKEY must not: it has no axes, and Metabase gives it two tabs, not three.
  it("gives an Axes tab to the charts that have axes, and not to the sankey", () => {
    for (const key of ["waterfall", "boxplot"]) {
      expect(byKey[key].controls.some((c) => c.section === "display" && c.group === "Axes")).toBe(true);
    }
    expect(byKey.sankey.controls.some((c) => c.group === "Axes")).toBe(false);
  });
});
