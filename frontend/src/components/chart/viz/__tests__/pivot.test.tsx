import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { createElement } from "react";
import { pivotDescriptor, type PivotConfig } from "../pivot";
import { EMPTY_SPEC } from "@/api/charts";
import type { VizControlProps } from "../types";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" }, orderby: [] };
const d = pivotDescriptor();

describe("pivotDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(d.requiredFields({ rows: [], cols: [], metrics: [] })).toEqual(["a row dimension", "a value"]);
  });

  it("builds a spec with rows then cols as dimensions and each group as a rollup", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: ["category"], metrics: ["total_revenue"] }, SHARED);
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.rollup_groups).toEqual([["region"], ["category"]]);
  });

  it("omits an empty group from the rollups", () => {
    const spec = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"] }, SHARED);
    expect(spec.rollup_groups).toEqual([["region"]]);
  });

  it("keeps interactive state out of the spec so collapsing never re-queries", () => {
    const a = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"] }, SHARED);
    const b = d.buildSpec({ rows: ["region"], cols: [], metrics: ["m"], collapsedRows: ["region:North"], sort: { region: "desc" } }, SHARED);
    expect(b).toEqual(a);
  });

  it("defaults the three totals toggles to on", () => {
    expect(d.defaultConfig.showRowTotals).toBe(true);
    expect(d.defaultConfig.showColTotals).toBe(true);
    expect(d.defaultConfig.condense).toBe(true);
  });

  it("reads a v1 params object from its loose top-level keys", () => {
    const cfg = d.fromParams({
      spec: EMPTY_SPEC, rows: ["region"], cols: [], metrics: ["m"],
      showRowTotals: false, collapsedRows: ["region:North"], pivotSort: { region: "desc" },
    });
    expect(cfg.rows).toEqual(["region"]);
    expect(cfg.showRowTotals).toBe(false);
    expect(cfg.collapsedRows).toEqual(["region:North"]);
    expect(cfg.sort).toEqual({ region: "desc" });
  });

  it("round-trips config → settings → config", () => {
    const cfg = {
      rows: ["region"], cols: ["category"], metrics: ["m"],
      showRowTotals: false, showColTotals: true, condense: true,
      collapsedRows: ["x"], sort: { region: "asc" as const },
    };
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });

  it("writes only namespaced keys", () => {
    const s = d.toSettings({ rows: ["region"], cols: [], metrics: ["m"] });
    expect(Object.keys(s).every((k) => k.startsWith("pivot."))).toBe(true);
  });
});

/**
 * THE TOTALS SWITCHES SAY WHAT THEY DO — "Show row totals", not "Row totals".
 *
 * Metabase's `pivot.show_row_totals` / `pivot.show_column_totals` are titled `Show row totals` and
 * `Show column totals` (`PivotTable/settings.ts`); egret dropped the verb. Behaviour matched on both
 * sides; this is the wording. It is not only parity: the third switch in the same group already
 * reads "Condense duplicate totals" — a verb — so two of the three named an OBJECT and one named an
 * ACTION, and next to a toggle a bare noun reads as a heading rather than as the thing the toggle
 * turns on.
 *
 * Asserted through the rendered control by the switch's ACCESSIBLE NAME, which is the string a
 * reader (and a screen reader) actually gets, and which `TotalsControl` derives from the same
 * `label` it prints — so a rename that reached only one of the two would fail here.
 */
describe("the pivot's totals switches, as the panel spells them", () => {
  const totals = d.controls.find((c) => c.id === "totals")!;
  const renderTotals = (config: Record<string, unknown> = {}) =>
    render(createElement(totals.render, {
      config: { rows: ["region"], cols: [], metrics: ["m"], ...config },
      onChange: () => {},
      columns: [], metrics: [], resultColumns: [], rows: [],
      temporalUnits: {}, unitRow: () => null,
    } as unknown as VizControlProps<PivotConfig>));

  it("names both totals switches after the action, the way the third one already did", () => {
    renderTotals();
    expect(screen.getByLabelText("Show row totals")).toBeInTheDocument();
    expect(screen.getByLabelText("Show column totals")).toBeInTheDocument();
    expect(screen.getByLabelText("Condense duplicate totals")).toBeInTheDocument();
  });

  it("leaves nothing behind under the old names", () => {
    renderTotals();
    expect(screen.queryByLabelText("Row totals")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Column totals")).not.toBeInTheDocument();
  });

  // The switches still drive the same stored keys — the rename is a label, not a key.
  it("still reads the stored flags it always did", () => {
    renderTotals({ showRowTotals: false, showColTotals: false });
    expect(screen.getByLabelText("Show row totals")).toHaveAttribute("data-state", "unchecked");
    expect(screen.getByLabelText("Show column totals")).toHaveAttribute("data-state", "unchecked");
  });
});
