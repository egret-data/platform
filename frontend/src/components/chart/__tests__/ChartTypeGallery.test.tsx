import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartTypeGallery } from "../ChartTypeGallery";

describe("ChartTypeGallery", () => {
  it("renders a card per type and calls onSelect on click", () => {
    const onSelect = vi.fn();
    render(<ChartTypeGallery onSelect={onSelect} />);
    expect(screen.getByText("Table")).toBeInTheDocument();
    expect(screen.getByText("Pivot Table")).toBeInTheDocument();
    expect(screen.getByText("Bar")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Pivot Table"));
    expect(onSelect).toHaveBeenCalledWith("pivot_table");
  });
});
