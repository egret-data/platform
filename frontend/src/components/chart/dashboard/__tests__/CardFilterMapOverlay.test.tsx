import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardFilterMapOverlay } from "../CardFilterMapOverlay";

describe("CardFilterMapOverlay", () => {
  it("shows the columns and writes the picked column", () => {
    const onChange = vi.fn();
    render(<CardFilterMapOverlay columns={["country", "city"]} value="" onChange={onChange} />);
    expect(screen.getByText("Column to filter on")).toBeTruthy();
    const sel = screen.getByLabelText("column to filter on") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "country", "city"]);
    fireEvent.change(sel, { target: { value: "city" } });
    expect(onChange).toHaveBeenCalledWith("city");
  });
  it("reflects the current mapping", () => {
    render(<CardFilterMapOverlay columns={["country"]} value="country" onChange={vi.fn()} />);
    expect((screen.getByLabelText("column to filter on") as HTMLSelectElement).value).toBe("country");
  });
  it("carries dash-nodrag so the grid does not hijack the select", () => {
    const { container } = render(<CardFilterMapOverlay columns={["country"]} value="" onChange={vi.fn()} />);
    expect(container.querySelector(".dash-nodrag")).toBeTruthy();
  });
});
