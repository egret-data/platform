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

describe("ProgressChart — Metabase parity", () => {
  const cols = [{ name: "m", type: "double" }, { name: "target", type: "double" }];

  // Metabase says so out loud; egret said nothing, leaving the reader to compare the bar against
  // the goal label to learn they had got there.
  it("says whether the goal was met, exceeded, or never set", () => {
    const { rerender } = render(<ProgressChart columns={cols} rows={[[120, 100]]} metric="m" goal={100} />);
    expect(screen.getByText("Goal exceeded")).toBeInTheDocument();
    rerender(<ProgressChart columns={cols} rows={[[100, 100]]} metric="m" goal={100} />);
    expect(screen.getByText("Goal met")).toBeInTheDocument();
    rerender(<ProgressChart columns={cols} rows={[[80, 100]]} metric="m" goal={100} />);
    expect(screen.queryByText(/Goal (met|exceeded)/)).toBeNull();
  });

  // KI-044: an empty Goal box stores nothing, and the bar used to read that as a goal of 0 —
  // "Goal 0" and "Goal exceeded" for any positive value, so "No goal set" could never appear.
  it("treats an empty goal as no goal, not as 0", () => {
    const { container } = render(<ProgressChart columns={[{ name: "m", type: "double" }]} rows={[[60]]} metric="m" />);
    expect(screen.getByText("No goal set")).toBeInTheDocument();
    expect(screen.queryByText(/Goal 0/)).toBeNull();
    expect(screen.queryByText("Goal exceeded")).toBeNull();
    expect(container.querySelector("[data-progress-pointer]")).toBeNull();
  });

  it("treats a goal column with no value as no goal", () => {
    const two = [{ name: "m", type: "double" }, { name: "target", type: "double" }];
    render(<ProgressChart columns={two} rows={[[60, null]]} metric="m" goalColumn="target" />);
    expect(screen.getByText("No goal set")).toBeInTheDocument();
    expect(screen.queryByText(/Goal 0/)).toBeNull();
  });

  it("reads its goal from a metric when one is chosen", () => {
    render(<ProgressChart columns={cols} rows={[[120, 500]]} metric="m" goal={100} goalColumn="target" />);
    // The column wins over the typed number, so 120 is now well short of 500.
    expect(screen.getByText("Goal 500")).toBeInTheDocument();
    expect(screen.queryByText("Goal exceeded")).toBeNull();
  });

  it("keeps the typed goal when the named column is not in the result", () => {
    render(<ProgressChart columns={cols} rows={[[120, 500]]} metric="m" goal={100} goalColumn="gone" />);
    expect(screen.getByText("Goal 100")).toBeInTheDocument();
  });

  // The track shades used to be two hexes copied from the default green. Derived from the bar
  // colour, they follow it — which is the whole point of making the colour configurable.
  it("derives its track from the bar colour rather than a fixed green", () => {
    const { container, rerender } = render(
      <ProgressChart columns={cols} rows={[[80, 100]]} metric="m" goal={100} color="#3366CC" />,
    );
    const track = container.querySelector("[data-progress-fill]")!.parentElement as HTMLElement;
    const blueTrack = track.style.backgroundColor;
    expect(blueTrack).not.toBe("");
    rerender(<ProgressChart columns={cols} rows={[[80, 100]]} metric="m" goal={100} color="#CC3333" />);
    const redTrack = (container.querySelector("[data-progress-fill]")!.parentElement as HTMLElement).style.backgroundColor;
    expect(redTrack).not.toBe(blueTrack);
  });
});

describe("ProgressChart — the message has its own row", () => {
  const cols = [{ name: "m", type: "double" }];

  // Measured against Metabase: it left-aligns the message on a row of its own. Appending it to the
  // floating value instead put the pair 76px past the right end of the bar and within 13px of the
  // card edge — a longer message would have gone over.
  it("does not ride along with the floating value label", () => {
    const { container } = render(<ProgressChart columns={cols} rows={[[120]]} metric="m" goal={100} />);
    const value = container.querySelector("[data-progress-value]")!;
    const message = container.querySelector("[data-progress-message]")!;
    expect(message).toBeInTheDocument();
    expect(value.contains(message)).toBe(false);
    // Pinned to the left edge, not to the fill end, and anchored by its BOTTOM to the bar's top —
    // a fixed negative offset let the descenders sit inside the bar.
    expect((message as HTMLElement).className).toContain("left-0");
    expect((message as HTMLElement).className).toContain("bottom-full");
  });
});
