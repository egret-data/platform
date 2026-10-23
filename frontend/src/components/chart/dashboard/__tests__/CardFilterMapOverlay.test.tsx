import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CardFilterMapOverlay } from "../CardFilterMapOverlay";
import {
  HEADER_COLUMN_TO_FILTER_ON, HINT_TEXT_CARD, NO_VALID_FIELDS, OTHER_TAB,
} from "../card-map-state";

const pick = { kind: "pick" as const, header: HEADER_COLUMN_TO_FILTER_ON, columns: ["country", "city"] };

describe("CardFilterMapOverlay", () => {
  it("shows the header and the columns and writes the picked column", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CardFilterMapOverlay state={pick} value="" onChange={onChange} />);
    expect(screen.getByText(HEADER_COLUMN_TO_FILTER_ON)).toBeTruthy();
    const sel = screen.getByRole("combobox", { name: "column to filter on" });
    expect(sel).toHaveTextContent("Select a column…");
    await user.click(sel);
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual(["country", "city"]);
    await user.click(screen.getByRole("option", { name: "city" }));
    expect(onChange).toHaveBeenCalledWith("city");
  });

  it("reflects the current mapping, and unmaps through No column", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<CardFilterMapOverlay state={pick} value="country" onChange={onChange} />);
    const sel = screen.getByRole("combobox", { name: "column to filter on" });
    expect(sel).toHaveTextContent("country");
    await user.click(sel);
    await user.click(await screen.findByRole("option", { name: "No column" }));
    expect(onChange).toHaveBeenCalledWith("");
  });

  it("gives the select the card's width instead of a fixed 192px", () => {
    // The measured defect: `w-48 max-w-full` resolved to 97px inside a 125px card at 1440px, which
    // is narrower than most column names and could not grow when the card did. Asserting the class
    // rather than a rendered width is deliberate — jsdom does no layout, so a width assertion here
    // would read 0 in every case and pass against anything. The NUMBER is asserted live, Task 9.
    render(<CardFilterMapOverlay state={pick} value="" onChange={vi.fn()} />);
    const sel = screen.getByLabelText("column to filter on");
    expect(sel.className).toContain("w-full");
    expect(sel.className).not.toContain("w-48");
  });

  it("omits the header when the state says there is no room for it", () => {
    render(<CardFilterMapOverlay state={{ ...pick, header: null }} value="" onChange={vi.fn()} />);
    expect(screen.queryByText(HEADER_COLUMN_TO_FILTER_ON)).toBeNull();
    expect(screen.getByLabelText("column to filter on")).toBeTruthy();
  });

  it("renders the reason, and NO select, whenever the card cannot take the filter", () => {
    for (const [state, text] of [
      [{ kind: "no-fields" as const, text: NO_VALID_FIELDS }, NO_VALID_FIELDS],
      [{ kind: "hint" as const, text: HINT_TEXT_CARD }, HINT_TEXT_CARD],
      [{ kind: "other-tab" as const, text: OTHER_TAB }, OTHER_TAB],
    ] as const) {
      const { unmount } = render(<CardFilterMapOverlay state={state} value="" onChange={vi.fn()} />);
      expect(screen.getByText(text)).toBeTruthy();
      expect(screen.queryByLabelText("column to filter on")).toBeNull();
      unmount();
    }
  });

  it("renders nothing at all for the none state", () => {
    const { container } = render(<CardFilterMapOverlay state={{ kind: "none" }} value="" onChange={vi.fn()} />);
    expect(container.firstChild).toBeNull();
  });

  it("carries dash-nodrag so the grid does not hijack the select", () => {
    const { container } = render(<CardFilterMapOverlay state={pick} value="" onChange={vi.fn()} />);
    expect(container.querySelector(".dash-nodrag")).toBeTruthy();
  });
});
