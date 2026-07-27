import { render, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { ChartEmptyState, ChartErrorState } from "../ChartPreviewStates";
import { BarChart3 } from "lucide-react";

describe("ChartPreviewStates", () => {
  it("empty state shows the title and hint", () => {
    render(<ChartEmptyState icon={BarChart3} title="Set up your Bar" hint="Add: X-axis, a metric" />);
    expect(screen.getByText("Set up your Bar")).toBeInTheDocument();
    expect(screen.getByText("Add: X-axis, a metric")).toBeInTheDocument();
  });
  it("error state shows the message and calls onRetry", async () => {
    const onRetry = vi.fn();
    render(<ChartErrorState message="boom" onRetry={onRetry} />);
    expect(screen.getByText("boom")).toBeInTheDocument();
    screen.getByRole("button", { name: "Retry" }).click();
    expect(onRetry).toHaveBeenCalled();
  });
});
