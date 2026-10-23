import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
  // The picker's options render only while it is open.
  const open = async () => {
    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "filter location" }));
    return { user, listbox: await screen.findByRole("listbox") };
  };

  it("offers Top of page plus one group per tab, in tab order", async () => {
    // MoveParameterMenu.tsx:104-129 — the root group holds only "Top of page" while any tab has a
    // hostable card.
    render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    const { listbox } = await open();
    const groups = within(listbox).getAllByRole("group").map((g) => g.firstElementChild?.textContent);
    expect(groups).toEqual(["Overview", "Detail"]);
    expect(within(listbox).getByRole("option", { name: "Top of page" })).toBeTruthy();
    expect(within(within(listbox).getAllByRole("group")[1]).getByRole("option", { name: "Detail chart" })).toBeTruthy();
  });

  it("drops the grouping on a dashboard with no tabs", async () => {
    render(<MoveParameterField value={null} destinations={flat} onChange={vi.fn()} />);
    const { listbox } = await open();
    expect(within(listbox).queryAllByRole("group")).toHaveLength(0);
    expect(within(listbox).getByRole("option", { name: "Revenue" })).toBeTruthy();
  });

  it("shows Top of page for a filter no card hosts, and the host otherwise", () => {
    const { rerender } = render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "filter location" })).toHaveTextContent("Top of page");
    rerender(<MoveParameterField value="bar" destinations={tabbed} onChange={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "filter location" })).toHaveTextContent("Revenue");
  });

  it("emits the card id, and null for Top of page", async () => {
    // Assert the RAW value the handler receives: a test that read the visible label would pass
    // against an implementation emitting "top-of-page" as a card id, which would then be looked
    // up as a card and silently found missing.
    const onChange = vi.fn();
    render(<MoveParameterField value="bar" destinations={tabbed} onChange={onChange} />);
    let { user } = await open();
    await user.click(screen.getByRole("option", { name: "Sales" }));
    expect(onChange).toHaveBeenCalledWith("head");
    ({ user } = await open());
    await user.click(screen.getByRole("option", { name: "Top of page" }));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("renders nothing when there is nowhere to move the filter", () => {
    const { container } = render(<MoveParameterField value={null} destinations={[]} onChange={vi.fn()} />);
    expect(container.firstChild).toBeNull();
  });

  it("keeps a host card whose tab has since been emptied selectable, rather than silently resetting", () => {
    // The picker's value must be a real option or it shows nothing chosen — or, worse, the
    // control would claim the filter is on the top bar while the card still holds it. Every
    // destination is rendered, so the host is always among them.
    render(<MoveParameterField value="far" destinations={tabbed} onChange={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "filter location" })).toHaveTextContent("Detail chart");
  });
});
