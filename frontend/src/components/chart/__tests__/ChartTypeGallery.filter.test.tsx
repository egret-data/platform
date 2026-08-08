import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartTypeGallery } from "../ChartTypeGallery";
import { CHART_TYPES } from "../chartTypes";

/**
 * WHAT THIS CAN AND CANNOT PIN.
 *
 * jsdom runs no layout engine, so the defect that made this change necessary — a 1246px dialog
 * centred in a 900px viewport at `top: -173px`, with its title, its close button and three of the
 * nineteen chart types off-screen and nothing anywhere able to scroll — is NOT observable here.
 * `getBoundingClientRect()` returns zeroes and Tailwind's `max-h-[calc(100dvh-4rem)]` resolves to
 * nothing. That half was measured in a real browser on the kind deployment and the numbers are in
 * the PR body.
 *
 * What a test CAN hold is the structure the fix depends on and the behaviour it added: that the
 * list is a scroll container rather than an unbounded grid, and that the filter genuinely narrows
 * nineteen items to the one you are after — including by DESCRIPTION, which is the half a reader
 * would not think to look for.
 */
describe("ChartTypeGallery — reaching one of nineteen", () => {
  it("filters by name", () => {
    render(<ChartTypeGallery onSelect={vi.fn()} />);
    expect(screen.getAllByRole("option")).toHaveLength(CHART_TYPES.length);
    fireEvent.change(screen.getByLabelText("Search chart types"), { target: { value: "sank" } });
    const shown = screen.getAllByRole("option");
    expect(shown).toHaveLength(1);
    expect(shown[0]).toHaveTextContent("Sankey");
  });

  /**
   * The reason the filter reads descriptions. Nobody looking for a distribution chart types "box
   * plot" — they type what they want, and the word is already on the card. A name-only filter would
   * answer "no chart type matches distribution" to a question the gallery can plainly answer.
   */
  it("filters by description, so a word that is on the card finds the card", () => {
    render(<ChartTypeGallery onSelect={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Search chart types"), { target: { value: "distribution" } });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(screen.getByRole("option")).toHaveTextContent("Box plot");
  });

  it("says so when nothing matches instead of showing an empty grid", () => {
    render(<ChartTypeGallery onSelect={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Search chart types"), { target: { value: "zzzz" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/No chart type matches/)).toBeInTheDocument();
  });

  it("still selects the type that is clicked, filtered or not", () => {
    const onSelect = vi.fn();
    render(<ChartTypeGallery onSelect={onSelect} />);
    fireEvent.change(screen.getByLabelText("Search chart types"), { target: { value: "pivot" } });
    fireEvent.click(screen.getByRole("option"));
    expect(onSelect).toHaveBeenCalledWith("pivot_table");
  });

  /**
   * The structural half of the overflow fix. `overflow-y-auto` is what turns the grid into a
   * scrollable region, and `min-h-0` is what lets it shrink inside the dialog's flex column — a
   * flex child without it refuses to go below its content height, which is exactly how the
   * overflow existed in the first place. Asserting the classes is a weak instrument and is said to
   * be one; it is here because jsdom offers no stronger one, and because losing either class
   * silently restores a dialog the user cannot reach the bottom of.
   */
  it("makes the list a scroll container that may shrink", () => {
    render(<ChartTypeGallery onSelect={vi.fn()} />);
    const list = screen.getByRole("listbox", { name: "Chart type" });
    expect(list.className).toContain("overflow-y-auto");
    expect(list.className).toContain("min-h-0");
  });
});
