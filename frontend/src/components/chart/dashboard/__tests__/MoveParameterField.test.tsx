import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MoveParameterField } from "../MoveParameterField";
import type { MoveDestination } from "../inlineParameters";

const tabbed: MoveDestination[] = [
  { cardId: "head", label: "Sales", tabName: "Overview" },
  { cardId: "bar", label: "Revenue", tabName: "Overview" },
  { cardId: "far", label: "Detail chart", tabName: "Detail" },
];
const flat: MoveDestination[] = [
  { cardId: "head", label: "Sales", tabName: null },
  { cardId: "bar", label: "Revenue", tabName: null },
];

describe("MoveParameterField", () => {
  it("offers Top of page plus one group per tab, in tab order", () => {
    // MoveParameterMenu.tsx:104-129 — the root group holds only "Top of page" while any tab has a
    // hostable card.
    render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    const groups = [...document.querySelectorAll("optgroup")].map((g) => g.label);
    expect(groups).toEqual(["Overview", "Detail"]);
    expect(screen.getByRole("option", { name: "Top of page" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "Detail chart" })).toBeTruthy();
  });

  it("drops the grouping on a dashboard with no tabs", () => {
    render(<MoveParameterField value={null} destinations={flat} onChange={vi.fn()} />);
    expect(document.querySelectorAll("optgroup")).toHaveLength(0);
    expect(screen.getByRole("option", { name: "Revenue" })).toBeTruthy();
  });

  it("shows Top of page for a filter no card hosts, and the host otherwise", () => {
    const { rerender } = render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    expect((screen.getByLabelText("filter location") as HTMLSelectElement).value).toBe("top-of-page");
    rerender(<MoveParameterField value="bar" destinations={tabbed} onChange={vi.fn()} />);
    expect((screen.getByLabelText("filter location") as HTMLSelectElement).value).toBe("bar");
  });

  it("emits the card id, and null for Top of page", () => {
    // Assert the RAW value the handler receives: a test that read the visible label would pass
    // against an implementation emitting "top-of-page" as a card id, which would then be looked
    // up as a card and silently found missing.
    const onChange = vi.fn();
    render(<MoveParameterField value="bar" destinations={tabbed} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("filter location"), { target: { value: "head" } });
    expect(onChange).toHaveBeenCalledWith("head");
    fireEvent.change(screen.getByLabelText("filter location"), { target: { value: "top-of-page" } });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("renders nothing when there is nowhere to move the filter", () => {
    const { container } = render(<MoveParameterField value={null} destinations={[]} onChange={vi.fn()} />);
    expect(container.firstChild).toBeNull();
  });

  it("keeps a host card whose tab has since been emptied selectable, rather than silently resetting", () => {
    // The `<select>`'s value must be a real option or the browser shows the FIRST one — which is
    // "Top of page", i.e. the control would claim the filter is on the top bar while the card
    // still holds it. Every destination is rendered, so the host is always among them.
    render(<MoveParameterField value="far" destinations={tabbed} onChange={vi.fn()} />);
    const sel = screen.getByLabelText("filter location") as HTMLSelectElement;
    expect(sel.value).toBe("far");
    expect(sel.selectedOptions[0].textContent).toBe("Detail chart");
  });
});
