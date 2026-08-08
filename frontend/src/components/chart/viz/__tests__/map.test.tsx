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

/**
 * ADDITIONAL TOOLTIP COLUMNS — the one chart family where Metabase's original feature ports as
 * itself. #433 established that it cannot exist on the cartesian charts, because those specs
 * GROUP BY the dimension and a raw column has no single value per drawn point; it substituted
 * dataset METRICS for that reason. A pin map takes `chartquery.rs`'s raw-records branch — no
 * GROUP BY at all — so a column's value on a pin is that pin's own row's cell, unambiguously.
 * These tests assert that the query actually asks for them, which is what makes the feature real
 * rather than a control that saves and does not paint.
 */
describe("mapDescriptor — additional tooltip columns", () => {
  const pin = { mapKey: "us_states", type: "pin" as const, latitude: "lat", longitude: "lon" };

  it("adds them to the SELECT, after the columns the map itself needs", () => {
    const spec = d.buildSpec({ ...pin, label: "city", tooltipColumns: ["rev", "orders"] }, SHARED);
    expect(spec.columns).toEqual(["lat", "lon", "city", "rev", "orders"]);
    // Still the raw-records branch: a stray metric would send it down the aggregate path and
    // collapse every pin sharing a coordinate into one.
    expect(spec.metrics).toEqual([]);
    expect(spec.dimensions).toEqual([]);
  });

  /** A duplicate name would emit `<col> AS <col>` twice in the SELECT, which the engine rejects
   *  outright — so the card would go from showing a column twice to showing nothing at all. */
  it("never selects a column twice", () => {
    const spec = d.buildSpec(
      { ...pin, label: "city", tooltipColumns: ["lat", "city", "rev", "rev"] }, SHARED,
    );
    expect(spec.columns).toEqual(["lat", "lon", "city", "rev"]);
  });

  it("round-trips through settings under Metabase's own key", () => {
    const cfg = { ...pin, tooltipColumns: ["rev"] };
    expect(d.toSettings(cfg)["map.tooltip_columns"]).toEqual(["rev"]);
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  /** Stored params are JSON that egret does not control the history of, and this is the only map
   *  key holding an ARRAY. A scalar where a list is expected would reach `buildSpec` and throw on
   *  `.filter`, taking the whole card down rather than the one setting. */
  it("ignores a stored value that is not a list of names", () => {
    const settings = { "map.key": "us_states", "map.type": "pin", "map.tooltip_columns": "rev" };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings }).tooltipColumns).toBeUndefined();
    const mixed = { ...settings, "map.tooltip_columns": ["rev", 7, null] };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: mixed }).tooltipColumns).toEqual(["rev"]);
  });

  it("is offered in pin mode and not in region mode", () => {
    // No label chosen, so `city` is still on offer and the control has something to say.
    const { unmount } = renderPanel(pin);
    expect(screen.getByLabelText("add tooltip columns")).toBeTruthy();
    unmount();
    renderPanel({ mapKey: "us_states", type: "region", region: "state", metric: "rev" });
    expect(screen.queryByLabelText("add tooltip columns")).toBeNull();
  });

  /** The coordinate and label columns are already on screen — the tooltip prints the pair on its
   *  own line and the label is its heading — so offering them would let a reader add a duplicate
   *  of something the pin already says. */
  it("does not offer a column the pin map already selects", async () => {
    renderPanel(pin);
    await userEvent.click(screen.getByLabelText("add tooltip columns"));
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toContain("city");
    expect(screen.getAllByRole("button").map((b) => b.textContent)).not.toContain("lat");
  });

  /**
   * HIDDEN ONLY WHEN THERE IS NOTHING TO OFFER **AND** NOTHING STORED — #433's rule. The second
   * clause is not belt-and-braces: egret never filters a stored setting at render time, so a
   * control hidden on availability alone would strand a value that still paints and still cannot
   * be removed. Both halves are asserted, because a fix for either alone passes the other test.
   */
  it("hides itself only when it has nothing to offer and nothing stored", () => {
    // Every column used by the map, nothing stored: nothing to say.
    const { unmount } = renderPanel({ ...pin, label: "city" });
    expect(screen.queryByLabelText("add tooltip columns")).toBeNull();
    expect(screen.queryByText("Tooltip columns")).toBeNull();
    unmount();
    // Every column used, but a stored value survives from before one of them was chosen — the
    // control must stay so the chip can be removed.
    renderPanel({ ...pin, label: "city", tooltipColumns: ["rev"] });
    expect(screen.getByText("Tooltip columns")).toBeTruthy();
    expect(screen.getByLabelText("remove rev")).toBeTruthy();
  });
});

/**
 * THE COORDINATE PICKERS DEFAULT FROM A DECLARED SEMANTIC TYPE — Metabase's own mechanism, which
 * #434 could not use and gave a reason for that turned out to be wrong. The reason on record was
 * that egret's `semantic_type` is "a backend enum plus the dataset column editor"; it is neither.
 * The migration adds `semantic_type text NOT NULL DEFAULT ''` with no CHECK, the Rust side types
 * it `String`, `ds_update_column` validates nothing, and `DatasetColumn` already carried it into
 * `VizControlProps.columns`. The whole change was two `<option>` elements and this default.
 *
 * IT IS NOT THE NAME-GUESS #434 REJECTED, and that distinction is what makes it safe to ship: a
 * heuristic on the column name is a guess about the data, and a semantic type is a statement
 * someone made about the data on purpose, in the same select that says a column is a URL.
 */
describe("mapDescriptor — the pin map's pickers default from the declared semantic type", () => {
  const declared = [
    col("city", "string"),
    { ...col("y", "numeric"), semantic_type: "latitude" },
    { ...col("x", "numeric"), semantic_type: "longitude" },
  ];
  /** Renders just the Map type select against a given column list, and reports what it patches. */
  function switchToPin(config: MapConfig, columns = declared) {
    const patches: Partial<MapConfig>[] = [];
    const ctl = d.controls.find((c) => c.id === "mapType")!;
    render(
      <ctl.render
        config={config} onChange={(p) => patches.push(p)} columns={columns} metrics={METRICS}
        resultColumns={[]} rows={[]} temporalUnits={{}} columnSettings={{}} unitRow={() => null}
      />,
    );
    return patches;
  }

  it("fills both coordinate fields when the dataset declares them", async () => {
    const patches = switchToPin({ mapKey: "us_states", type: "region" });
    await userEvent.click(screen.getByLabelText("Map type"));
    await userEvent.click(screen.getByRole("option", { name: "Pin map" }));
    expect(patches).toEqual([{ type: "pin", latitude: "y", longitude: "x" }]);
  });

  /** BOTH OR NEITHER. One coordinate is not a location, and pairing a declared column with one the
   *  user picked for something else is the confident-wrong-hemisphere failure by another route. */
  it("fills neither when only one is declared", async () => {
    const patches = switchToPin({ mapKey: "us_states", type: "region" }, [declared[0], declared[1]]);
    await userEvent.click(screen.getByLabelText("Map type"));
    await userEvent.click(screen.getByRole("option", { name: "Pin map" }));
    expect(patches).toEqual([{ type: "pin" }]);
  });

  /** A declaration cannot overwrite a choice — the user's own pick wins, in either slot. */
  it("leaves an existing choice alone", async () => {
    const patches = switchToPin({ mapKey: "us_states", type: "region", latitude: "mine" });
    await userEvent.click(screen.getByLabelText("Map type"));
    await userEvent.click(screen.getByRole("option", { name: "Pin map" }));
    expect(patches).toEqual([{ type: "pin" }]);
  });

  /** A text column that calls itself a latitude is still a text column, and `PinControls` would
   *  not offer it — defaulting to it would put a value in a picker that is not in its own list. */
  it("ignores a declared column that is not numeric", async () => {
    const text = [{ ...col("lat_name", "string"), semantic_type: "latitude" }, declared[2]];
    const patches = switchToPin({ mapKey: "us_states", type: "region" }, text);
    await userEvent.click(screen.getByLabelText("Map type"));
    await userEvent.click(screen.getByRole("option", { name: "Pin map" }));
    expect(patches).toEqual([{ type: "pin" }]);
  });

  /**
   * SWITCHING BACK IS NOT A DEFAULTING MOMENT — it must patch nothing but the type, or a region map
   * would carry coordinate columns it never uses into its stored settings.
   *
   * The config here has NO coordinates, which is what makes the assertion bite: a mutation that
   * applied the defaults on both branches survived the earlier version of this test, because that
   * one started from a config whose latitude was already set and so had nothing to default anyway.
   */
  it("writes nothing extra when switching back to a region map", async () => {
    const patches = switchToPin({ mapKey: "us_states", type: "pin" });
    await userEvent.click(screen.getByLabelText("Map type"));
    await userEvent.click(screen.getByRole("option", { name: "Region map" }));
    expect(patches).toStrictEqual([{ type: "region" }]);
  });
});
