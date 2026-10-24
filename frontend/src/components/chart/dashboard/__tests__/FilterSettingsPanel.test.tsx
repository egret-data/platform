import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("FilterSettingsPanel tier2d", () => {
  it("type select offers boolean and location", async () => {
    const user = userEvent.setup();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "parameter type" }));
    const opts = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(opts).toContain("True / False");
    expect(opts).toContain("Location");
  });

  it("widget radio writes param.widget", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("widget search"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ widget: "search" }));
  });

  it("default control writes param.default", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} defaultOptions={["West"]}
      onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("option West"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: ["West"] }));
  });

  it("clearing the default writes undefined", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({ default: ["West"] })} allParameters={[p({})]} defaultOptions={["West"]}
      onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "clear default value" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: undefined }));
  });

  it("linked-filters checkbox writes filteringParameters with other param ids", () => {
    const onChange = vi.fn();
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({})} allParameters={[p({}), other]} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("link City"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ filteringParameters: ["2"] }));
  });

  it("does not offer the param itself as a linked filter", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("link Region")).toBeNull();
  });
});

describe("FilterSettingsPanel base", () => {
  it("edits name and required, and deletes", () => {
    const onChange = vi.fn();
    const onDelete = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={onDelete} />);
    fireEvent.change(screen.getByLabelText("parameter name"), { target: { value: "Reg" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ name: "Reg" }));
    fireEvent.click(screen.getByLabelText("parameter required"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ required: true }));
    fireEvent.click(screen.getByRole("button", { name: "delete filter" }));
    expect(onDelete).toHaveBeenCalled();
  });

  it("changing type resets the operator to the first for that type", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "parameter type" }));
    await user.click(await screen.findByRole("option", { name: "Number" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ type: "number", operator: "=" }));
  });

  it("clicking delete filter fires onDelete", () => {
    const onDelete = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={onDelete} />);
    fireEvent.click(screen.getByLabelText("delete filter"));
    expect(onDelete).toHaveBeenCalled();
  });

  it("offers People can pick for a category, and writes the RAW flag", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    expect(screen.getByLabelText("people can pick multiple")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("people can pick single"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ isMultiSelect: false }));
  });

  it("hides People can pick where egret has no widget that could honour it", () => {
    // date is absent from Metabase's table; text is present but rendered as a single input here.
    render(<FilterSettingsPanel param={p({ type: "date", operator: "range" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("people can pick multiple")).toBeNull();
  });

  it("hides People can pick when the widget is an input box", () => {
    render(<FilterSettingsPanel param={p({ widget: "input" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("people can pick multiple")).toBeNull();
  });

  it("uses Metabase's heading and explains an empty linked-filters list", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText("Limit this filter's choices")).toBeTruthy();
    expect(screen.getByText(/So first, add another dashboard filter/)).toBeTruthy();
  });

  it("says why a custom-sourced filter cannot be linked, instead of showing an empty list", () => {
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({ valuesSource: { type: "static", static: ["EU"] } })}
      allParameters={[p({}), other]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText(/can't be limited by another dashboard filter/)).toBeTruthy();
    expect(screen.queryByLabelText("link City")).toBeNull();
  });

  it("says why an input-box filter cannot be linked", () => {
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({ widget: "input" })} allParameters={[p({}), other]}
      onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText(/its widget type is an input box/)).toBeTruthy();
  });

  it("offers the move control between Required and Delete filter", async () => {
    const user = userEvent.setup();
    // Metabase's position (ParameterSettings.tsx:293): after the default/required block, before
    // the destructive actions. Asserted by DOM order, not by eye.
    const onMove = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()}
      moveDestinations={[{ cardId: "bar", label: "Revenue", tabName: null }]} hostCardId={null} onMove={onMove} />);
    const order = [...document.querySelectorAll("[aria-label]")].map((e) => e.getAttribute("aria-label"));
    expect(order.indexOf("filter location")).toBeGreaterThan(order.indexOf("parameter required"));
    expect(order.indexOf("filter location")).toBeLessThan(order.indexOf("delete filter"));
    await user.click(screen.getByRole("combobox", { name: "filter location" }));
    await user.click(await screen.findByRole("option", { name: "Revenue" }));
    expect(onMove).toHaveBeenCalledWith("bar");
  });

  it("hides the move control on a dashboard with no card that could host a filter", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()}
      moveDestinations={[]} hostCardId={null} onMove={vi.fn()} />);
    expect(screen.queryByLabelText("filter location")).toBeNull();
  });

  it("hides the move control entirely for a caller that has not been wired for it", () => {
    // Every other W6 test in this file renders without `onMove`. If the control appeared anyway
    // it would be a picker whose changes go nowhere.
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()}
      moveDestinations={[{ cardId: "bar", label: "Revenue", tabName: null }]} />);
    expect(screen.queryByLabelText("filter location")).toBeNull();
  });
});

// KI-235: the panel opens where the card settings panel does, so it is built
// from the same parts: labelled fields, the shared Select, switches for on/off
// rows and radio groups for one-of choices. Never the browser's own controls.
describe("FilterSettingsPanel parts", () => {
  it("uses the card settings panel's controls, and no browser select or radio", () => {
    const other = p({ id: "2", name: "City", slug: "city" });
    const { container } = render(
      <FilterSettingsPanel param={p({ type: "text", operator: "is" })} allParameters={[p({}), other]}
        onChange={vi.fn()} onDelete={vi.fn()}
        moveDestinations={[{ cardId: "bar", label: "Revenue", tabName: null }]} hostCardId={null} onMove={vi.fn()} />,
    );
    expect(container.querySelector("select")).toBeNull();
    expect(container.querySelector('input[type="radio"]')).toBeNull();
    expect(screen.getByLabelText("parameter name").getAttribute("data-slot")).toBe("input");
    for (const name of ["parameter type", "parameter operator", "filter location"]) {
      expect(screen.getByRole("combobox", { name }).getAttribute("data-slot")).toBe("select-trigger");
    }
    expect(screen.getByRole("switch", { name: "parameter required" })).toBeTruthy();
    expect(screen.getByRole("switch", { name: "link City" })).toBeTruthy();
    expect(screen.getByRole("radiogroup", { name: "How should people filter on this?" })).toBeTruthy();
  });
});
