import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SeriesOrderSection } from "../SeriesOrderSection";

describe("SeriesOrderSection", () => {
  it("moving the last series up swaps it with its neighbor", () => {
    const onChange = vi.fn();
    render(<SeriesOrderSection names={["a", "b", "c"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Move c up"));
    expect(onChange).toHaveBeenCalledWith({ seriesOrder: ["a", "c", "b"] });
  });

  it("toggling a series' switch off emits hiddenSeries containing it", () => {
    const onChange = vi.fn();
    render(<SeriesOrderSection names={["a", "b"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Show b"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSeries: ["b"] });
  });

  it("toggling a hidden series' switch back on removes it from hiddenSeries", () => {
    const onChange = vi.fn();
    render(<SeriesOrderSection names={["a", "b"]} hidden={["b"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Show b"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSeries: [] });
  });

  it("ranks by the stored order, keeping unranked names after it in their natural place", () => {
    render(<SeriesOrderSection names={["a", "b", "c"]} order={["c", "a"]} onChange={() => {}} />);
    const rows = screen.getAllByRole("switch").map((el) => el.getAttribute("aria-label"));
    expect(rows).toEqual(["Show c", "Show a", "Show b"]);
  });

  it("disables the up-arrow on the first row and the down-arrow on the last", () => {
    render(<SeriesOrderSection names={["a", "b"]} onChange={() => {}} />);
    expect(screen.getByLabelText("Move a up")).toBeDisabled();
    expect(screen.getByLabelText("Move b down")).toBeDisabled();
    expect(screen.getByLabelText("Move a down")).not.toBeDisabled();
    expect(screen.getByLabelText("Move b up")).not.toBeDisabled();
  });
});
