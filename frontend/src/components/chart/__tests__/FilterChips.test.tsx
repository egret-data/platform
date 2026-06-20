import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterChips } from "../FilterChips";

describe("FilterChips", () => {
  it("renders a chip per filter and removes on ×", () => {
    const onRemove = vi.fn();
    render(<FilterChips filters={[{ col: "region", op: "eq", value: "EU" }]} onRemove={onRemove} />);
    expect(screen.getByText(/region/)).toBeInTheDocument();
    expect(screen.getByText(/EU/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /remove filter/i }));
    expect(onRemove).toHaveBeenCalledWith(0);
  });
  it("renders nothing when empty", () => {
    const { container } = render(<FilterChips filters={[]} onRemove={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
