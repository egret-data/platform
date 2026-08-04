import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SeriesLineStyleSection } from "../SeriesLineStyleSection";

describe("SeriesLineStyleSection", () => {
  it("renders a style and size control per series name", () => {
    render(<SeriesLineStyleSection names={["a", "b"]} onChange={() => {}} />);
    expect(screen.getByLabelText("style a")).toBeInTheDocument();
    expect(screen.getByLabelText("size a")).toBeInTheDocument();
    expect(screen.getByLabelText("style b")).toBeInTheDocument();
    expect(screen.getByLabelText("size b")).toBeInTheDocument();
  });

  it("choosing a style emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} onChange={onChange} />);
    await user.click(screen.getByLabelText("style a"));
    await user.click(await screen.findByRole("option", { name: "Dashed" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { style: "dashed" } } });
  });

  it("choosing a size emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} onChange={onChange} />);
    await user.click(screen.getByLabelText("size a"));
    await user.click(await screen.findByRole("option", { name: "L" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { size: "L" } } });
  });

  // Pins the bug the shape invites: seriesLine is a map, so setting one series' field must
  // MERGE, not replace — get the outer spread wrong and it wipes every other series; get the
  // inner spread wrong and it wipes this series' other field.
  it("setting one series' style preserves the other series' entry and this series' size", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection
        names={["a", "b"]}
        seriesLine={{ a: { style: "dashed" }, b: { size: "L" } }}
        onChange={onChange}
      />,
    );
    await user.click(screen.getByLabelText("style b"));
    await user.click(await screen.findByRole("option", { name: "Dotted" }));
    expect(onChange).toHaveBeenCalledWith({
      seriesLine: { a: { style: "dashed" }, b: { size: "L", style: "dotted" } },
    });
  });

  it("choosing Auto for style clears it back to undefined", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} seriesLine={{ a: { style: "dashed" } }} onChange={onChange} />);
    await user.click(screen.getByLabelText("style a"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { style: undefined } } });
  });
});

/**
 * `shape` and `markers` were declared on `LineSeriesStyle` and honoured by `decorateSeries`
 * (echarts/options/cartesian.ts) from the day the per-series map landed, but nothing ever
 * rendered a control for them — a chart drew what a user could not ask for. The omission was
 * argued rather than accidental (a third and fourth Select per row would have made an unbounded
 * per-series list worse); the drill-in removes that premise, so the fields become reachable.
 *
 * The patch shape is asserted MERGED against an existing entry, not just present: the same outer
 * and inner spread the style/size tests already pin, because a wrong spread here silently wipes a
 * setting the chart is still drawing.
 */
describe("SeriesLineStyleSection — shape and markers", () => {
  it("choosing a shape emits it under that series' key, merged over its other fields", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection names={["a", "b"]} seriesLine={{ a: { size: "L" }, b: { style: "dotted" } }} onChange={onChange} />,
    );
    await user.click(screen.getByLabelText("shape a"));
    await user.click(await screen.findByRole("option", { name: "Step" }));
    expect(onChange).toHaveBeenCalledWith({
      seriesLine: { a: { size: "L", shape: "step" }, b: { style: "dotted" } },
    });
  });

  it("choosing markers emits it under that series' key, merged over its other fields", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection names={["a", "b"]} seriesLine={{ a: { size: "L" }, b: { style: "dotted" } }} onChange={onChange} />,
    );
    await user.click(screen.getByLabelText("markers b"));
    await user.click(await screen.findByRole("option", { name: "Off" }));
    expect(onChange).toHaveBeenCalledWith({
      seriesLine: { a: { size: "L" }, b: { style: "dotted", markers: "off" } },
    });
  });

  // "Auto" is the only way back to the chart-wide default. Writing the literal string "auto"
  // instead of `undefined` would pin the series to a value the builder then reads as an override,
  // and `cfg.lineShape` would stop reaching it forever.
  it("Auto clears shape and markers back to undefined so the chart-wide default applies", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} seriesLine={{ a: { shape: "step", markers: "off" } }} onChange={onChange} />);
    await user.click(screen.getByLabelText("shape a"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenLastCalledWith({ seriesLine: { a: { shape: undefined, markers: "off" } } });
    await user.click(screen.getByLabelText("markers a"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenLastCalledWith({ seriesLine: { a: { shape: "step", markers: undefined } } });
  });

  // The trigger must show what the BUILDER will resolve, not a blank. A stored value that the
  // control cannot display is how a user learns their setting was lost.
  it("shows the stored shape and markers on the trigger", () => {
    render(<SeriesLineStyleSection names={["a"]} seriesLine={{ a: { shape: "smooth", markers: "on" } }} onChange={() => {}} />);
    expect(screen.getByLabelText("shape a")).toHaveTextContent("Smooth");
    expect(screen.getByLabelText("markers a")).toHaveTextContent("On");
  });
});
