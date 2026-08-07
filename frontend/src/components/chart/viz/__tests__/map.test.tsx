import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mapDescriptor } from "../map";
import { EMPTY_SPEC } from "@/api/charts";
import type { MapConfig } from "@/api/charts";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: {}, orderby: [] };
const d = mapDescriptor();

const col = (column_name: string, type_generic: string): DatasetColumn => ({
  column_name, data_type: type_generic === "numeric" ? "double" : "varchar", type_generic,
  is_temporal: false, ordinal: 0, expression: null, verbose_name: "", description: "",
  is_calculated: false, groupby: true, filterable: true, format: "", semantic_type: "",
});
const COLUMNS = [col("city", "string"), col("lat", "numeric"), col("lon", "numeric")];
const METRICS: DatasetMetric[] = [];

/** Renders every control the descriptor declares, for one config — what the builder's panel shows. */
function renderPanel(config: MapConfig) {
  return render(
    <>
      {d.controls.map((c) => (
        <c.render
          key={c.id} config={config} onChange={() => {}} columns={COLUMNS} metrics={METRICS}
          resultColumns={[]} rows={[]} temporalUnits={{}} columnSettings={{}} unitRow={() => null}
        />
      ))}
    </>,
  );
}

describe("mapDescriptor", () => {
  it("names both missing fields", () => {
    expect(d.requiredFields({ mapKey: "us_states" })).toEqual(["a region", "a metric"]);
  });

  it("defaults to the us_states map", () => {
    expect(d.defaultConfig.mapKey).toBe("us_states");
  });

  it("builds a spec grouping by the region column", () => {
    const spec = d.buildSpec({ mapKey: "world", region: "country", metric: "total_revenue" }, SHARED);
    expect(spec.dimensions).toEqual(["country"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("reads a v1 map chart from its nested key", () => {
    // `type: "region"` is supplied by `fromParams`, not stored: v1 params predate pin mode, so the
    // nested blob can only ever describe a region map.
    expect(d.fromParams({ spec: EMPTY_SPEC, map: { mapKey: "world", region: "country", metric: "m" } }))
      .toEqual({ mapKey: "world", type: "region", region: "country", metric: "m" });
  });

  it("round-trips config → settings → config", () => {
    // `type` joined this config in the pin-map change and is always written, so it is named here
    // too — a round-trip fixture that omitted it would be asserting against a config the
    // descriptor never produces.
    const cfg = { mapKey: "vietnam", type: "region" as const, region: "province", metric: "m" };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });
});

/**
 * THE PIN MAP — Metabase's `map.type: "pin"`, individual markers at latitude/longitude.
 *
 * The load-bearing claim here is that a pin map needs NO BACKEND CHANGE, because `QuerySpec`
 * already has a raw-records mode: `crates/egret-bi/src/chartquery.rs` emits
 * `SELECT <col> AS <col> FROM … WHERE … ORDER BY … LIMIT` with no GROUP BY whenever `metrics` is
 * empty and `columns` is not. So the whole query half of this feature is one branch in `buildSpec`
 * that asks for columns instead of an aggregate — and that is what these assertions pin, in the
 * shape the backend actually keys on rather than by naming the SQL.
 */
describe("mapDescriptor — pin map", () => {
  it("asks the backend for RAW RECORDS, not an aggregate", () => {
    const spec = d.buildSpec({ mapKey: "us_states", type: "pin", latitude: "lat", longitude: "lon" }, SHARED);
    // The backend's raw-records branch is chosen by `metrics` being empty AND `columns` not being;
    // either half wrong and it emits a GROUP BY that collapses every pin at a shared coordinate.
    expect(spec.metrics).toEqual([]);
    expect(spec.dimensions).toEqual([]);
    expect(spec.columns).toEqual(["lat", "lon"]);
  });

  it("also fetches the label column, so a pin can say what it is", () => {
    const spec = d.buildSpec(
      { mapKey: "us_states", type: "pin", latitude: "lat", longitude: "lon", label: "city" }, SHARED,
    );
    expect(spec.columns).toEqual(["lat", "lon", "city"]);
  });

  /** A half-configured pin map must not fire a query: `columns: []` with `metrics: []` is the one
   *  combination the backend rejects outright ("a table needs at least one metric or column"), so
   *  the preview has to ask for the field instead of showing a 400. */
  it("names the missing coordinate fields rather than querying without them", () => {
    expect(d.requiredFields({ mapKey: "us_states", type: "pin" })).toEqual(["a latitude", "a longitude"]);
    expect(d.requiredFields({ mapKey: "us_states", type: "pin", latitude: "lat" })).toEqual(["a longitude"]);
    expect(d.requiredFields({ mapKey: "us_states", type: "pin", latitude: "lat", longitude: "lon" })).toEqual([]);
  });

  /** A pin map has no region and no metric, so demanding them would make it permanently unrunnable
   *  — the region branch's requirement must not leak across. */
  it("does not demand a region or a metric", () => {
    expect(d.requiredFields({ mapKey: "us_states", type: "pin", latitude: "lat", longitude: "lon" }))
      .not.toContain("a region");
  });

  it("round-trips the pin config through settings", () => {
    const cfg = {
      mapKey: "world", type: "pin" as const, latitude: "lat", longitude: "lon", label: "city",
    };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  /**
   * A CHART SAVED BEFORE THIS FEATURE HAS NO `map.type`, and it is a region map. Reading it as
   * anything else would silently re-point every existing map chart at a query it cannot run.
   */
  it("reads a map saved before pin mode existed as a region map", () => {
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: { "map.key": "us_states", "map.region": "state", "map.metric": "rev" } }).type).toBe("region");
    expect(d.fromParams({ spec: EMPTY_SPEC, map: { mapKey: "world", region: "country", metric: "m" } }).type).toBe("region");
  });
});

/**
 * THE PANEL IS HALF THE FEATURE. Metabase hides Region and Metric when the map type is "pin" and
 * hides the coordinate fields when it is "region" (`getHidden: vizSettings["map.type"] !== …`), so
 * a reader is never offered a field that does nothing. A control that saves and does not paint —
 * or is offered and cannot apply — is the defect this programme has closed repeatedly, so both
 * directions are asserted rather than only the new one.
 */
describe("mapDescriptor — the panel offers only the fields its map type uses", () => {
  it("offers Latitude and Longitude in pin mode, and not Region or Metric", () => {
    renderPanel({ mapKey: "us_states", type: "pin" });
    expect(screen.getByLabelText("Latitude")).toBeTruthy();
    expect(screen.getByLabelText("Longitude")).toBeTruthy();
    expect(screen.queryByLabelText("Region")).toBeNull();
    expect(screen.queryByLabelText("Metric")).toBeNull();
  });

  it("offers Region and Metric in region mode, and not the coordinate fields", () => {
    renderPanel({ mapKey: "us_states", type: "region" });
    expect(screen.getByLabelText("Region")).toBeTruthy();
    expect(screen.getByLabelText("Metric")).toBeTruthy();
    expect(screen.queryByLabelText("Latitude")).toBeNull();
    expect(screen.queryByLabelText("Longitude")).toBeNull();
  });

  /** Both types paint with the same accent — the Color control is not a region-only setting, and
   *  leaving it out of pin mode would be a pin map with no way to change the pin colour. */
  it("keeps the Map and Color controls in both modes", () => {
    renderPanel({ mapKey: "us_states", type: "pin" });
    expect(screen.getByLabelText("Map")).toBeTruthy();
    expect(screen.getByLabelText("Color")).toBeTruthy();
  });

  /**
   * ONLY NUMERIC COLUMNS ARE OFFERED AS COORDINATES — Metabase's `fieldFilter: isNumeric` on both
   * `map.latitude_column` and `map.longitude_column`. A varchar city name in the Latitude picker
   * would produce a chart of zero pins and no error.
   */
  it("offers only numeric columns as coordinates", async () => {
    renderPanel({ mapKey: "us_states", type: "pin" });
    await userEvent.click(screen.getByLabelText("Latitude"));
    // `city` is a varchar and is deliberately absent; asserting the WHOLE list rather than the
    // absence of one name, so a filter that let anything else through fails here too.
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["lat", "lon"]);
  });

  /** The label is free to be any column — it is what the pin SAYS, not where it goes — so the
   *  numeric filter must not leak onto it and hide every name a reader would want. */
  it("offers every column as the label, numeric or not", async () => {
    renderPanel({ mapKey: "us_states", type: "pin" });
    await userEvent.click(screen.getByLabelText("Label"));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toContain("city");
  });
});
