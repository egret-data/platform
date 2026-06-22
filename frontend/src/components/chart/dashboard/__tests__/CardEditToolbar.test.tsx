import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardEditToolbar } from "../CardEditToolbar";

describe("CardEditToolbar", () => {
  it("calls each action; shows replace only for chart cards", () => {
    const fns = { onSettings: vi.fn(), onDuplicate: vi.fn(), onReplace: vi.fn(), onRemove: vi.fn() };
    const { rerender } = render(<CardEditToolbar cardType="chart" {...fns} />);
    fireEvent.click(screen.getByRole("button", { name: "card settings" })); expect(fns.onSettings).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "duplicate card" })); expect(fns.onDuplicate).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "replace card" })); expect(fns.onReplace).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "remove card" })); expect(fns.onRemove).toHaveBeenCalled();
    rerender(<CardEditToolbar cardType="text" {...fns} />);
    expect(screen.queryByRole("button", { name: "replace card" })).toBeNull();
  });
});
