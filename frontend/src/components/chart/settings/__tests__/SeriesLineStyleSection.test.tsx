import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SeriesLineStyleSection } from "../SeriesLineStyleSection";

/**
 * This section renders ONE series — it is the body of that series' drill-in popover. It used to
 * render the whole list, one row per series, which is why these tests used to pass `names={[...]}`
 * and assert two series' controls side by side. They now render the section twice where the point
 * is that two series do not interfere, and the "one series' controls do not leak into another's
 * popover" case lives in SeriesListSection.test.tsx, where the popover actually is.
 */
describe("SeriesLineStyleSection", () => {
  it("renders all five of LineSeriesStyle for the series it is given", () => {
    render(<SeriesLineStyleSection name="a" onChange={() => {}} />);
    for (const f of ["shape", "style", "size", "markers", "missing"]) {
      expect(screen.getByLabelText(`${f} a`)).toBeInTheDocument();
    }
  });

  it("choosing a style emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection name="a" onChange={onChange} />);
    await user.click(screen.getByLabelText("style a"));
    await user.click(await screen.findByRole("option", { name: "Dashed" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { style: "dashed" } } });
  });

  it("choosing a size emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection name="a" onChange={onChange} />);
    await user.click(screen.getByLabelText("size a"));
    await user.click(await screen.findByRole("option", { name: "L" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { size: "L" } } });
  });

  // Pins the bug the shape invites: seriesLine is a map, so setting one series' field must
  // MERGE, not replace — get the outer spread wrong and it wipes every other series (which this
  // section can no longer even see, making the mistake easier, not harder); get the inner spread
  // wrong and it wipes this series' other fields.
  it("setting one series' style preserves the other series' entry and this series' size", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection name="b" seriesLine={{ a: { style: "dashed" }, b: { size: "L" } }} onChange={onChange} />,
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
    render(<SeriesLineStyleSection name="a" seriesLine={{ a: { style: "dashed" } }} onChange={onChange} />);
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
      <SeriesLineStyleSection name="a" seriesLine={{ a: { size: "L" }, b: { style: "dotted" } }} onChange={onChange} />,
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
      <SeriesLineStyleSection name="b" seriesLine={{ a: { size: "L" }, b: { style: "dotted" } }} onChange={onChange} />,
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
    render(<SeriesLineStyleSection name="a" seriesLine={{ a: { shape: "step", markers: "off" } }} onChange={onChange} />);
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
    render(<SeriesLineStyleSection name="a" seriesLine={{ a: { shape: "smooth", markers: "on" } }} onChange={() => {}} />);
    expect(screen.getByLabelText("shape a")).toHaveTextContent("Smooth");
    expect(screen.getByLabelText("markers a")).toHaveTextContent("On");
  });
});

/**
 * `missing` IS THE FIFTH FIELD AND THE LAST TO ARRIVE, because `LineSeriesStyle` deliberately did
 * not carry it. The argument, on `decorateSeries`, was that the other four are appearance while
 * `missing: "zero"` REWRITES THE DATA — "and that zero then counts in stack totals, in
 * normalisation and in a trend fit" — so two bands disagreeing would make the chart say two things
 * at once. Measuring it ended it: the stack total and every other band's normalised share are
 * byte-identical across all three values, and the trend fit that really does move is fitted per
 * series off that series' own data. `cartesian.perSeriesMissing` holds the three numbers.
 *
 * The wording here is egret's chart-wide control's ("Missing values", Connect / Zero / Gap), not
 * Metabase's ("Replace missing values with", Zero / Nothing / Linear Interpolated), for the reason
 * the `markers` row already gives: Auto on this select FALLS BACK to that control, and two names
 * for one setting is how a reader stops believing the fallback exists.
 */
describe("SeriesLineStyleSection — missing values", () => {
  it("choosing one emits it under that series' key, merged over its other fields", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection name="b" seriesLine={{ a: { size: "L" }, b: { style: "dotted" } }} onChange={onChange} />,
    );
    await user.click(screen.getByLabelText("missing b"));
    await user.click(await screen.findByRole("option", { name: "Zero" }));
    expect(onChange).toHaveBeenCalledWith({
      seriesLine: { a: { size: "L" }, b: { style: "dotted", missing: "zero" } },
    });
  });

  it("offers exactly the three the builder resolves, plus Auto", async () => {
    const user = userEvent.setup();
    render(<SeriesLineStyleSection name="a" onChange={() => {}} />);
    await user.click(screen.getByLabelText("missing a"));
    // Not a superset check: a fourth option here would be a value `decorateSeries` silently ignores.
    expect((await screen.findAllByRole("option")).map((o) => o.textContent))
      .toEqual(["Auto", "Connect", "Zero", "Gap"]);
  });

  it("Auto clears it back to undefined so the chart-wide value applies", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection name="a" seriesLine={{ a: { missing: "zero" } }} onChange={onChange} />);
    await user.click(screen.getByLabelText("missing a"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { missing: undefined } } });
  });

  it("shows the stored value on the trigger", () => {
    render(<SeriesLineStyleSection name="a" seriesLine={{ a: { missing: "none" } }} onChange={() => {}} />);
    expect(screen.getByLabelText("missing a")).toHaveTextContent("Gap");
  });
});
