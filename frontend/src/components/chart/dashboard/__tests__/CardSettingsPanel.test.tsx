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
  it("maps a parameter to a column", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("map Region"), { target: { value: "region" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ paramMappings: { pr: "region" } });
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
});

describe("CardSettingsPanel (text card)", () => {
  it("edits the text body", () => {
    const onUpdateOptions = vi.fn();
    const c: DraftCard = { id: "t1", chart_id: null, card_type: "text", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("text body"), { target: { value: "hello" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ text: "hello" });
  });
});
