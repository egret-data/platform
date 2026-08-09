import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterApplyBar } from "../FilterApplyBar";

describe("FilterApplyBar", () => {
  it("stays out of the way while auto-apply is on", () => {
    render(<FilterApplyBar autoApply changedCount={2} onApply={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  it("stays out of the way when nothing has changed", () => {
    render(<FilterApplyBar autoApply={false} changedCount={0} onApply={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  it("counts the changes and wires both buttons", () => {
    const onApply = vi.fn();
    const onCancel = vi.fn();
    render(<FilterApplyBar autoApply={false} changedCount={2} onApply={onApply} onCancel={onCancel} />);
    expect(screen.getByText("2 filters changed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApply).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });
});
