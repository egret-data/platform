import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardSettingsPanel } from "../CardSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "../tabs";

const param: DashboardParameter = { id: "pr", name: "Region", slug: "region", type: "category", operator: "is" };
const chart = { id: "ch", dataset_id: "d", viz_type: "bar", name: "Sales", params: { spec: { dimensions: ["region"], columns: [], metrics: ["m"], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const card = (opts = {}): DraftCard => ({ id: "c1", chart_id: "ch", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: opts, tab_id: null, tabKey: "ta" });
const tabs: DraftTab[] = [{ key: "ta", name: "A", position: 0 }, { key: "tb", name: "B", position: 1 }];

describe("CardSettingsPanel (chart card)", () => {
  it("edits title", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("card title"), { target: { value: "Hi" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ title: "Hi" });
  });
  it("no longer shows the per-param mapping (moved to in-card overlay)", () => {
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText(/^map /)).toBeNull();
  });
  it("sets on-click to cross-filter", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("on click"), { target: { value: "crossfilter" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ clickBehavior: { type: "crossfilter", parameterMapping: {} } });
  });
  it("moves the card to another tab", () => {
    const onMoveToTab = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={onMoveToTab} />);
    fireEvent.change(screen.getByLabelText("move card to tab"), { target: { value: "tb" } });
    expect(onMoveToTab).toHaveBeenCalledWith("tb");
  });
  it("turns on hide-when-empty and writes only that key", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("hide when empty"));
    // toStrictEqual, not toEqual: the point of this assertion is that the patch carries
    // NOTHING but hideEmpty, and toEqual/toHaveBeenCalledWith would pass on a patch that
    // also set an undefined sibling key.
    expect(onUpdateOptions).toHaveBeenCalledTimes(1);
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ hideEmpty: true });
  });
  it("shows hide-when-empty already on, and turns it back off", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card({ hideEmpty: true })} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("hide when empty")).toBeChecked();
    fireEvent.click(screen.getByLabelText("hide when empty"));
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ hideEmpty: false });
  });
  it("uses Metabase's exact wording, since this panel is measured against its modal", () => {
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByText("Hide this card if there are no results")).toBeInTheDocument();
  });
});

describe("CardSettingsPanel (text card)", () => {
  it("shows the {{slug}} hint and lists available parameter slugs", () => {
    const c: DraftCard = { id: "t1", chart_id: null, card_type: "text", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByText(/Insert a filter value/i)).toBeInTheDocument();
    expect(screen.getByText("{{region}}")).toBeInTheDocument();
  });
  it("offers no hide-when-empty toggle, because a text card runs no query", () => {
    // Metabase draws the same line: getHidden: ([{ card }]) => isVirtualDashCard(card).
    const c: DraftCard = { id: "t1", chart_id: null, card_type: "text", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("hide when empty")).toBeNull();
  });
});
