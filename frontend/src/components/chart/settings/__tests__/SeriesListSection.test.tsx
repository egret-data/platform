import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SeriesListSection } from "../SeriesListSection";

describe("SeriesListSection", () => {
  it("moving the last series up swaps it with its neighbor", () => {
    const onChange = vi.fn();
    render(<SeriesListSection names={["a", "b", "c"]} defaultColor={() => "#000000"} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Move c up"));
    expect(onChange).toHaveBeenCalledWith({ seriesOrder: ["a", "c", "b"] });
  });

  it("toggling a series' switch off emits hiddenSeries containing it", () => {
    const onChange = vi.fn();
    render(<SeriesListSection names={["a", "b"]} defaultColor={() => "#000000"} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Show b"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSeries: ["b"] });
  });

  it("toggling a hidden series' switch back on removes it from hiddenSeries", () => {
    const onChange = vi.fn();
    render(<SeriesListSection names={["a", "b"]} hidden={["b"]} defaultColor={() => "#000000"} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Show b"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSeries: [] });
  });

  it("ranks by the stored order, keeping unranked names after it in their natural place", () => {
    render(<SeriesListSection names={["a", "b", "c"]} order={["c", "a"]} defaultColor={() => "#000000"} onChange={() => {}} />);
    const rows = screen.getAllByRole("switch").map((el) => el.getAttribute("aria-label"));
    expect(rows).toEqual(["Show c", "Show a", "Show b"]);
  });

  it("disables the up-arrow on the first row and the down-arrow on the last", () => {
    render(<SeriesListSection names={["a", "b"]} defaultColor={() => "#000000"} onChange={() => {}} />);
    expect(screen.getByLabelText("Move a up")).toBeDisabled();
    expect(screen.getByLabelText("Move b down")).toBeDisabled();
    expect(screen.getByLabelText("Move a down")).not.toBeDisabled();
    expect(screen.getByLabelText("Move b up")).not.toBeDisabled();
  });

  it("reordering visible series does not forget a series absent from this run", () => {
    const onChange = vi.fn();
    // "b" is in the stored order but not in this run's names (e.g. filtered out).
    render(<SeriesListSection names={["a", "c"]} order={["a", "b", "c"]} defaultColor={() => "#000000"} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Move c up"));
    const [{ seriesOrder }] = onChange.mock.calls[0];
    expect(seriesOrder).toContain("b");
    expect(seriesOrder.indexOf("c")).toBeLessThan(seriesOrder.indexOf("a"));
  });
});

describe("SeriesListSection — one row per series", () => {
  const base = { names: ["a", "b"], defaultColor: () => "#000000" };

  // Colour and rename used to live in a SECOND list under the same heading, so two series filled
  // four rows and each name was printed twice.
  it("carries colour, rename, order and visibility on the same row", () => {
    const onChange = vi.fn();
    render(<SeriesListSection {...base} onChange={onChange} />);
    for (const label of ["Move a down", "color a", "label a", "Show a"]) {
      expect(screen.getByLabelText(label)).toBeInTheDocument();
    }
    fireEvent.change(screen.getByLabelText("color b"), { target: { value: "#ff0000" } });
    expect(onChange).toHaveBeenCalledWith({ seriesColors: { b: "#ff0000" } });
  });

  it("renames and clears the rename when emptied", () => {
    const onChange = vi.fn();
    render(<SeriesListSection {...base} labels={{ a: "Old" }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("label a"), { target: { value: "New" } });
    expect(onChange).toHaveBeenCalledWith({ seriesLabels: { a: "New" } });
    fireEvent.change(screen.getByLabelText("label a"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ seriesLabels: {} });
  });

  // The two old lists disagreed: colours drew in natural order, order/visibility drew ranked. A
  // swatch therefore stopped belonging to the name beside it as soon as anything was moved. The
  // default colour still has to follow the series' NATURAL slot, which is the one it is drawn in.
  it("keeps a reordered series' default colour tied to the slot the renderer paints it in", () => {
    const palette = ["#111111", "#222222", "#333333"];
    render(
      <SeriesListSection
        names={["a", "b", "c"]} order={["c", "a", "b"]}
        defaultColor={(i) => palette[i]} onChange={() => {}}
      />,
    );
    const rows = screen.getAllByRole("switch").map((el) => el.getAttribute("aria-label"));
    expect(rows).toEqual(["Show c", "Show a", "Show b"]);
    expect((screen.getByLabelText("color c") as HTMLInputElement).value).toBe("#333333");
    expect((screen.getByLabelText("color a") as HTMLInputElement).value).toBe("#111111");
  });
});
