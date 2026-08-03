import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { SIMPLE_DESCRIPTORS } from "../simple";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import type { VizControlProps } from "../types";

/**
 * THE SLICE LIST READS IN THE ORDER THE PIE DRAWS.
 *
 * `PieDisplayControl` used to hand the panel the raw DISTINCT dimension values, in row order, with
 * no measure at all — so the list could neither sort by value (the pie's own order) nor reproduce
 * the "Other" fold. Both are asserted here, through the descriptor's own control and against
 * `buildPieOption`'s actual output, rather than against a restatement of the rule: a panel test that
 * is handed a pre-ordered `slices` array proves the section renders what it is given and says
 * nothing about what the app gives it. (PR 02 found the same gap on the cartesian side.)
 */
describe("the pie's slice list, through the descriptor's own control", () => {
  const pieDescriptor = SIMPLE_DESCRIPTORS.find((d) => d.key === "pie")!;
  const control = pieDescriptor.controls.find((c) => c.section === "display")!;
  const cols = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];

  /**
   * THE FIXTURE IS NOT PRE-SORTED, and that is the whole point of it: rows arrive
   * Gizmo, Widget, Gadget, Doohickey while the pie draws Widget, Gadget, Gizmo, Doohickey. A
   * fixture whose rows already arrive value-descending cannot detect a sorting bug at all — it
   * passes against the broken control, which simply echoed the row order back.
   *
   * The values are also chosen so the fold has something to bite on: at a 26% threshold Gizmo (20%)
   * and Doohickey (10%) are both under it, which is 2+ slices and so a real "Other" bucket, while at
   * 15% only Doohickey is — and `buildSlices` declines to fold a single slice (Metabase's merge-2+
   * rule).
   */
  const rows: unknown[][] = [["Gizmo", 20], ["Widget", 40], ["Gadget", 30], ["Doohickey", 10]];
  const RAW_ORDER = ["Gizmo", "Widget", "Gadget", "Doohickey"];

  const base = { dimensions: ["category"], metric: "m" };

  const renderPanel = (cfg: Record<string, unknown> = {}) =>
    render(createElement(control.render, {
      config: { ...base, ...cfg },
      onChange: () => {},
      columns: [], metrics: [],
      resultColumns: cols,
      rows,
      unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));

  /** Ring 0 exactly as the chart draws it: the sunburst's top-level data, in array order. */
  const ring0 = (cfg: Record<string, unknown> = {}) =>
    (buildPieOption(cols, rows, { ...base, ...cfg }) as unknown as {
      series: { data: { name: string; itemStyle: { color: string } }[] }[];
    }).series[0].data;
  const drawnOrder = (cfg: Record<string, unknown> = {}) => ring0(cfg).map((d) => d.name);
  const drawnColors = (cfg: Record<string, unknown> = {}) =>
    Object.fromEntries(ring0(cfg).map((d) => [d.name, d.itemStyle.color.toLowerCase()]));

  /** The panel's rows, in the order they are rendered — read off the one control every row has. */
  const panelRows = () =>
    screen.getAllByLabelText(/^Move .+ up$/).map((el) => el.getAttribute("aria-label")!.slice(5, -3));
  const swatch = (name: string) =>
    (screen.getByLabelText(`color ${name}`) as HTMLInputElement).value.toLowerCase();

  it("lists the slices in the order the pie draws them, not the order the rows arrived", () => {
    const drawn = drawnOrder();
    expect(drawn).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    expect(drawn).not.toEqual(RAW_ORDER); // the fixture can tell the two apart
    renderPanel();
    expect(panelRows()).toEqual(drawn);
  });

  // "Sort by value" off is the pie drawing its rows in the order they arrived, and the list has to
  // follow the CHART rather than sort by value on its own account.
  it("follows the chart back to row order when Sort by value is off", () => {
    expect(drawnOrder({ sortRows: false })).toEqual(RAW_ORDER);
    renderPanel({ sortRows: false });
    expect(panelRows()).toEqual(RAW_ORDER);
  });

  // A stored manual position is applied on top of the chart's order, by both sides — so the rows
  // the user did not move stay value-descending underneath it.
  it("applies a stored sliceOrder on top of the drawn order, as the chart does", () => {
    const cfg = { sliceOrder: ["Doohickey"] };
    expect(drawnOrder(cfg)).toEqual(["Doohickey", "Widget", "Gadget", "Gizmo"]);
    renderPanel(cfg);
    expect(panelRows()).toEqual(drawnOrder(cfg));
  });

  /**
   * A FOLDED SLICE HAS NO ACTIONABLE ROW. Its colour, its name and its visibility are all decided by
   * the "Other" bucket now, so a row offering those three is a row of controls that do nothing.
   * Metabase renders nothing at all for a row it marked `isOther` (ChartSettingOrderedItems'
   * `renderItem` returns null).
   */
  it("drops a folded slice's row rather than leaving it with controls that do nothing", () => {
    const cfg = { sliceThreshold: 26 };
    expect(drawnOrder(cfg)).toEqual(["Widget", "Gadget", "Other"]); // 2 slices folded, so a bucket exists
    renderPanel(cfg);
    expect(panelRows()).toEqual(["Widget", "Gadget"]);
    for (const gone of ["Gizmo", "Doohickey"]) {
      expect(screen.queryByLabelText(`Show ${gone}`)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(`color ${gone}`)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(`label ${gone}`)).not.toBeInTheDocument();
    }
  });

  // …and the fold is reported, so the reader can see where the two missing categories went. It is
  // not a row: the bucket takes no curation on either side (the builder identifies it by a sentinel
  // that no stored key can carry).
  it("names the Other bucket and what went into it, without offering it as a row", () => {
    renderPanel({ sliceThreshold: 26 });
    expect(screen.getByText(/^Other/)).toHaveTextContent("Gizmo");
    expect(screen.getByText(/^Other/)).toHaveTextContent("Doohickey");
    expect(screen.queryByLabelText("Show Other")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Move Other up")).not.toBeInTheDocument();
  });

  /** `buildSlices` refuses to fold a single sub-threshold slice — so the panel must not drop one
   *  either. A list that simply hid everything under the threshold would fail this. */
  it("keeps the row of a lone sub-threshold slice, which the chart does not fold", () => {
    const cfg = { sliceThreshold: 15 }; // only Doohickey (10%) is under it
    expect(drawnOrder(cfg)).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    renderPanel(cfg);
    expect(panelRows()).toEqual(drawnOrder(cfg));
  });

  /**
   * THE COLOUR CONSEQUENCE OF THE FOLD, which is what made this more than a cosmetic list bug.
   * A folded slice is not a ring-0 key, so its `sliceColors` pin never reaches `colorsForKeys` and
   * never consumes that colour — the chart hands it to whoever hashes onto it. The panel, listing
   * the folded slice as a live key, spent the slot and named a different colour for a slice that
   * was not folded at all.
   */
  it("does not spend a colour on a folded slice, so the survivors' swatches are the drawn ones", () => {
    const cfg = { sliceThreshold: 26, sliceColors: { Gizmo: "#f2a86f" } };
    const chart = drawnColors(cfg);
    expect(chart["Widget"]).toBe("#f2a86f"); // the premise: the pin freed nothing, Widget takes it
    renderPanel(cfg);
    for (const name of ["Widget", "Gadget"]) expect(swatch(name)).toBe(chart[name]);
  });

  /** The arrows walk the LISTED rows. Indexing them against a list that still counted the folded
   *  slices would leave the last row's ↓ live and swap it with a slice that has no row. */
  it("ends the arrows at the last listed row, not at the last slice in the data", () => {
    const onChange = vi.fn();
    render(createElement(control.render, {
      config: { ...base, sliceThreshold: 26 },
      onChange, columns: [], metrics: [], resultColumns: cols, rows, unitRow: () => null,
    } as VizControlProps<Record<string, unknown>>));
    expect(screen.getByLabelText("Move Widget up")).toBeDisabled();
    expect(screen.getByLabelText("Move Gadget down")).toBeDisabled();
    fireEvent.click(screen.getByLabelText("Move Widget down"));
    expect(onChange).toHaveBeenCalledWith({ sliceOrder: ["Gadget", "Widget"] });
  });

  // A hidden slice is not drawn, but it must stay listed — its own switch is the only way back.
  it("still lists a hidden slice, which is the only route back to showing it", () => {
    renderPanel({ hiddenSlices: ["Widget"] });
    expect(panelRows()).toContain("Widget");
    expect((screen.getByLabelText("Show Widget") as HTMLInputElement).getAttribute("data-state"))
      .toBe("unchecked");
  });
});
