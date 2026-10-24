import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
  // KI-064: the box says "Blank = chart name", but clearing it stored `title: ""`, which is how a
  // card hides its caption, so the caption vanished instead of falling back.
  it("clearing the title falls back to the chart name rather than hiding the caption", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card({ title: "x" })} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("card title"), { target: { value: "" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ title: undefined });
  });
  it("hides the caption only through Show title (KI-064)", () => {
    const onUpdateOptions = vi.fn();
    const { rerender } = render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    const show = screen.getByLabelText("show title");
    expect(show.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(show);
    expect(onUpdateOptions).toHaveBeenCalledWith({ title: "" });
    // A hidden caption has no title to edit, and switching it back on returns to the chart name.
    rerender(<CardSettingsPanel card={card({ title: "" })} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("card title")).toBeNull();
    fireEvent.click(screen.getByLabelText("show title"));
    expect(onUpdateOptions).toHaveBeenLastCalledWith({ title: undefined });
  });
  it("no longer shows the per-param mapping (moved to in-card overlay)", () => {
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText(/^map /)).toBeNull();
  });
  it("sets on-click to cross-filter", async () => {
    const user = userEvent.setup();
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "on click" }));
    await user.click(await screen.findByRole("option", { name: "Cross-filter" }));
    expect(onUpdateOptions).toHaveBeenCalledWith({ clickBehavior: { type: "crossfilter", parameterMapping: {} } });
  });
  it("moves the card to another tab", async () => {
    const user = userEvent.setup();
    const onMoveToTab = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={onMoveToTab} />);
    await user.click(screen.getByRole("combobox", { name: "move card to tab" }));
    await user.click(await screen.findByRole("option", { name: "B" }));
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

  it("edits the per-card description", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("card description"), { target: { value: "Net of refunds." } });
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ description: "Net of refunds." });
  });

  it("shows the background on by default for a chart card and writes only that key", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("show background")).toBeChecked();
    fireEvent.click(screen.getByLabelText("show background"));
    // toStrictEqual, for the same reason the hide-when-empty case gives: the point is that the
    // patch carries NOTHING but background.
    expect(onUpdateOptions).toHaveBeenCalledTimes(1);
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ background: false });
  });

  it("uses Metabase's wording for the background toggle", () => {
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByText("Show background")).toBeInTheDocument();
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
  it("offers the background toggle, reading OFF for a heading", () => {
    const c: DraftCard = { id: "h1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("show background")).not.toBeChecked();
  });
});

describe("CardSettingsPanel (iframe card)", () => {
  const iframeCard = (opts: Record<string, unknown> = {}): DraftCard => ({
    id: "if1", chart_id: null, card_type: "iframe", grid_col: 0, grid_row: 0,
    size_x: 12, size_y: 8, options: opts, tab_id: null, tabKey: null,
  });

  it("edits the single free-text embed setting and writes only that key", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("iframe url or embed code"), {
      target: { value: "https://www.loom.com/share/abc" },
    });
    // toStrictEqual, not toEqual: the point is that the patch carries NOTHING else.
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ iframe: "https://www.loom.com/share/abc" });
  });

  it("shows the stored value", () => {
    render(<CardSettingsPanel card={iframeCard({ iframe: "https://figma.com/embed?x=1" })} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("iframe url or embed code")).toHaveValue("https://figma.com/embed?x=1");
  });

  it("offers no auto-height toggle, because a cross-origin frame cannot be measured", () => {
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("auto height")).toBeNull();
  });

  it("offers no hide-when-empty toggle, because an iframe card runs no query", () => {
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("hide when empty")).toBeNull();
  });

  it("DOES offer the background toggle, on by default", () => {
    // W5 generalised "Show background" to every card type, and its own comment names iframe as
    // one of the four Metabase offers it on. `showsCardBackground` defaults anything that is not
    // a heading or a placeholder to true, so an iframe card must read checked — the branch added
    // here has to carry the shared row, not quietly drop it.
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("show background")).toBeChecked();
    fireEvent.click(screen.getByLabelText("show background"));
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ background: false });
  });

  it("does NOT fall through to the chart-card panel", () => {
    // Without its own branch, an iframe card lands on the default return at the bottom of the
    // file and gets a Title field and an On-click select, neither of which applies.
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("on click")).toBeNull();
  });
});
