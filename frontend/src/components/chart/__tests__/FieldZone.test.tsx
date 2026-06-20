import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FieldZone } from "../config/FieldZone";

describe("FieldZone", () => {
  it("renders a chip per selected field and removes on ×", () => {
    const onChange = vi.fn();
    render(<FieldZone label="Rows" selected={["a", "b"]} options={["a", "b", "c"]} onChange={onChange} />);
    expect(screen.getByText("a")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: /remove/i })[0]);
    expect(onChange).toHaveBeenCalledWith(["b"]);
  });
  it("adds a field via the + Add affordance", () => {
    const onChange = vi.fn();
    render(<FieldZone label="Rows" selected={["a"]} options={["a", "b", "c"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /add rows/i }));
    fireEvent.click(screen.getByText("b"));
    expect(onChange).toHaveBeenCalledWith(["a", "b"]);
  });
});
