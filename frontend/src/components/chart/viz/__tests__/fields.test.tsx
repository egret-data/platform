import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { fieldDescriptor } from "../fields";
import { EMPTY_SPEC } from "@/api/charts";
import type { DatasetColumn } from "@/api/datasets";

const d = fieldDescriptor({
  key: "funnel",
  prefix: "funnel.",
  fields: [
    { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
    { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
  ],
  buildSpec: (cfg, shared, helpers) => helpers.dimsAndMetrics([cfg.dimension], [cfg.metric], shared),
  legacyKey: "funnel",
  legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
});

describe("fieldDescriptor", () => {
  it("derives requiredFields from the field list, in order", () => {
    expect(d.requiredFields({})).toEqual(["a dimension", "a metric"]);
    expect(d.requiredFields({ dimension: "region" })).toEqual(["a metric"]);
    expect(d.requiredFields({ dimension: "region", metric: "m" })).toEqual([]);
  });

  it("writes namespaced settings and reads them back", () => {
    const cfg = { dimension: "region", metric: "m" };
    expect(d.toSettings(cfg)).toEqual({ "funnel.dimension": "region", "funnel.metric": "m" });
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("reads a v1 chart from its nested key", () => {
    expect(d.fromParams({ spec: EMPTY_SPEC, funnel: { dimension: "region", metric: "m" } }))
      .toEqual({ dimension: "region", metric: "m" });
  });

  it("falls back to the v1 spec when the nested key is missing", () => {
    const cfg = d.fromParams({ spec: { ...EMPTY_SPEC, dimensions: ["region"], metrics: ["m"] } });
    expect(cfg).toEqual({ dimension: "region", metric: "m" });
  });

  // Same failure mode as the table's: a v2 row that never got this viz's settings slice used to
  // open on the "set up your chart" placeholder with the fields sitting in the spec.
  it("falls back to the spec when the v2 settings have no slice for this prefix", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, dimensions: ["stage"], metrics: ["users"] },
      settings: { "cartesian.xAxis": "someone else's key" },
    });
    expect(cfg).toEqual({ dimension: "stage", metric: "users" });
  });

  it("prefers the stored slice over the spec", () => {
    const cfg = d.fromParams({
      version: 2,
      spec: { ...EMPTY_SPEC, dimensions: ["from_spec_dim"], metrics: ["from_spec_m"] },
      settings: { "funnel.dimension": "stored_dim", "funnel.metric": "stored_m" },
    });
    expect(cfg).toEqual({ dimension: "stored_dim", metric: "stored_m" });
  });

  // A brand-new chart has an empty slice AND an empty spec; it must stay `{}` rather than gain
  // keys holding nothing, which is what the panel and the run guard read.
  //
  // `toStrictEqual`, not `toEqual`: `{ dimension: undefined }` deep-equals `{}` under `toEqual`,
  // so a recovery that handed back undefined-valued keys passed the first version of this test.
  it("stays empty for a v2 chart with nothing picked yet", () => {
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: {} })).toStrictEqual({});
  });

  // The other half of the same guard: a field holding a LIST recovers `[]` from an empty spec,
  // and an empty array is neither undefined nor pruned by `toStrictEqual`.
  it("stays empty for a v2 chart whose fields are lists", () => {
    const withList = fieldDescriptor({
      key: "pie",
      prefix: "pie.",
      fields: [
        { id: "dimensions", label: "Dimension", kind: "dimensions", required: "a dimension", max: 2 },
        { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      ],
      buildSpec: (cfg, shared, helpers) =>
        helpers.dimsAndMetrics((cfg.dimensions as string[]) ?? [], [cfg.metric], shared),
      legacyKey: "pie",
      legacyFromSpec: (spec) => ({ dimensions: spec.dimensions, metric: spec.metrics?.[0] }),
    });
    expect(withList.fromParams({ version: 2, spec: EMPTY_SPEC, settings: {} })).toStrictEqual({});
    // …but a spec that names something still comes back.
    expect(withList.fromParams({
      version: 2, spec: { ...EMPTY_SPEC, dimensions: ["category"], metrics: ["orders"] }, settings: {},
    })).toStrictEqual({ dimensions: ["category"], metric: "orders" });
  });

  it("builds a spec from the declared fields", () => {
    const spec = d.buildSpec({ dimension: "region", metric: "m" }, { filters: [], rowLimit: 500, temporalUnits: {} });
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.metrics).toEqual(["m"]);
    expect(spec.row_limit).toBe(500);
  });

  it("drops an unset field from the spec instead of emitting an empty string", () => {
    const spec = d.buildSpec({ dimension: "region" }, { filters: [], rowLimit: 100, temporalUnits: {} });
    expect(spec.metrics).toEqual([]);
  });

  it("gives every declared field a data control", () => {
    expect(d.controls.map((c) => c.id)).toEqual(["dimension", "metric"]);
    expect(d.controls.every((c) => c.section === "data")).toBe(true);
  });
});

function col(name: string, temporal = false): DatasetColumn {
  return {
    column_name: name, data_type: temporal ? "timestamp" : "varchar", type_generic: temporal ? "temporal" : "string",
    is_temporal: temporal, ordinal: 0, expression: null, verbose_name: name, description: "",
    is_calculated: false, groupby: true, filterable: true, format: "", semantic_type: "",
  };
}

const COLUMNS = [col("region"), col("category"), col("city"), col("order_date", true), col("ship_date", true)];

describe("fieldDescriptor: the `dimensions` field kind (multi-select, up to `max`)", () => {
  const multi = fieldDescriptor({
    key: "pie",
    prefix: "pie2.",
    fields: [
      { id: "dimensions", label: "Dimensions", kind: "dimensions", max: 3, required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (cfg, shared, helpers) =>
      helpers.dimsAndMetrics(Array.isArray(cfg.dimensions) ? cfg.dimensions : [], [cfg.metric], shared),
  });
  const Control = multi.controls.find((c) => c.id === "dimensions")!.render;

  function renderControl(config: Record<string, unknown>, onChange = vi.fn()) {
    render(
      <Control
        config={config}
        onChange={onChange}
        columns={COLUMNS}
        metrics={[]}
        resultColumns={[]}
        rows={[]}
        unitRow={(c) => (c ? <div data-testid={`unit-${c}`}>unit:{c}</div> : null)}
      />,
    );
    return onChange;
  }

  it("renders one empty picker when nothing is chosen", () => {
    renderControl({});
    expect(screen.getAllByRole("combobox")).toHaveLength(1);
  });

  it("emits the whole array with the picked column", async () => {
    const onChange = renderControl({});
    await userEvent.click(screen.getByRole("combobox"));
    await userEvent.click(await screen.findByRole("option", { name: "region" }));
    expect(onChange).toHaveBeenCalledWith({ dimensions: ["region"] });
  });

  it("shows a second empty picker once one dimension is chosen", () => {
    renderControl({ dimensions: ["region"] });
    expect(screen.getAllByRole("combobox")).toHaveLength(2);
  });

  it("does not offer an already-chosen column in the remaining pickers", async () => {
    renderControl({ dimensions: ["region"] });
    const boxes = screen.getAllByRole("combobox");
    await userEvent.click(boxes[1]); // the empty "add" picker
    expect(screen.queryByRole("option", { name: "region" })).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: "category" })).toBeInTheDocument();
  });

  it("renders no further picker once `max` is reached", () => {
    renderControl({ dimensions: ["region", "category", "city"] });
    expect(screen.getAllByRole("combobox")).toHaveLength(3);
  });

  it("keeps the remaining two in order after removing the first of three", async () => {
    const onChange = renderControl({ dimensions: ["region", "category", "city"] });
    await userEvent.click(screen.getByRole("button", { name: /remove region/i }));
    expect(onChange).toHaveBeenCalledWith({ dimensions: ["category", "city"] });
  });

  it("always emits the full array, never a partial patch, when replacing an existing pick", async () => {
    const onChange = renderControl({ dimensions: ["region", "category"] });
    const boxes = screen.getAllByRole("combobox");
    await userEvent.click(boxes[0]);
    await userEvent.click(await screen.findByRole("option", { name: "city" }));
    expect(onChange).toHaveBeenCalledWith({ dimensions: ["city", "category"] });
  });

  // Design decision (task 2, step 3): `temporalUnits` is keyed by column name, and every
  // other multi-column control (pivot's rows/cols, table's columns) renders the shared
  // date-grouping row once per selected column, not just the first — so a second temporal
  // dimension silently losing its grouping control would be a regression relative to that
  // precedent. Pin that this field follows the same rule.
  it("renders the date-grouping row for every chosen temporal dimension, not only the first", () => {
    renderControl({ dimensions: ["order_date", "ship_date"] });
    expect(screen.getByTestId("unit-order_date")).toBeInTheDocument();
    expect(screen.getByTestId("unit-ship_date")).toBeInTheDocument();
  });

  // `[]` is truthy in JS — a plain `!cfg[f.id]` check would call an emptied-out dimensions
  // list "set", so a user who removes their last pick would stop seeing "Add: a dimension"
  // even though the field is once again unconfigured.
  it("still reports the field as required after its array is emptied out", () => {
    expect(multi.requiredFields({ dimensions: [], metric: "m" })).toEqual(["a dimension"]);
    expect(multi.requiredFields({ dimensions: ["region"], metric: "m" })).toEqual([]);
  });
});
