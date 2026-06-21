import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ProgressChart } from "../ProgressChart";

const cols = [{ name: "rev", type: "double" }];

describe("ProgressChart", () => {
  it("shows the value and goal and fills the bar to value/goal", () => {
    const { container } = render(<ProgressChart columns={cols} rows={[[60]]} metric="rev" goal={100} format={{ decimals: 0 }} />);
    expect(screen.getByText("60")).toBeInTheDocument();
    expect(screen.getByText(/Goal\s*100/)).toBeInTheDocument();
    expect(container.querySelector("[data-progress-fill]")).toHaveStyle({ width: "60%" });
  });
  it("when the goal is exceeded the fill is goal/value (overshoot visible)", () => {
    const { container } = render(<ProgressChart columns={cols} rows={[[200]]} metric="rev" goal={100} />);
    expect(container.querySelector("[data-progress-fill]")).toHaveStyle({ width: "50%" });
  });
  it("renders an em dash when there is no row", () => {
    render(<ProgressChart columns={cols} rows={[]} metric="rev" goal={100} />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
