import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardEditToolbar } from "../CardEditToolbar";

describe("CardEditToolbar", () => {
  it("calls each action; shows replace only for chart cards", () => {
    const fns = { onViz: vi.fn(), onDuplicate: vi.fn(), onReplace: vi.fn(), onRemove: vi.fn() };
    const { rerender } = render(<CardEditToolbar cardType="chart" {...fns} />);
    expect(screen.queryByRole("button", { name: "card settings" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "duplicate card" })); expect(fns.onDuplicate).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "card visualization" })); expect(fns.onViz).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "replace card" })); expect(fns.onReplace).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "remove card" })); expect(fns.onRemove).toHaveBeenCalled();
    rerender(<CardEditToolbar cardType="text" {...fns} />);
    expect(screen.queryByRole("button", { name: "replace card" })).toBeNull();
    expect(screen.queryByRole("button", { name: "card visualization" })).toBeNull();
  });
  it("carries the dash-nodrag class so react-grid-layout doesn't hijack toolbar clicks", () => {
    const fns = { onViz: vi.fn(), onDuplicate: vi.fn(), onReplace: vi.fn(), onRemove: vi.fn() };
    const { container } = render(<CardEditToolbar cardType="chart" {...fns} />);
    expect(container.querySelector(".dash-nodrag")).toBeTruthy();
  });
});
