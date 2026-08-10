import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SeriesListSection, reorderSeriesOrder } from "../SeriesListSection";

describe("SeriesListSection", () => {
  /**
   * Driven through the pure helper: reordering is a drag now, and no test in this repo drives a dnd
   * drop — dnd-kit's keyboard sensor reads element rects and jsdom has none. What the DOM can still
   * be asked is that every row has a grip, which the last case below does.
   */
  it("a move writes the order the chart will draw", () => {
    expect(reorderSeriesOrder(undefined, ["a", "b", "c"], "c", "b")).toEqual(["a", "c", "b"]);
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

  it("gives every row a grip, and no row a disabled one — a drag has no ends to hit", () => {
    render(<SeriesListSection names={["a", "b"]} onChange={() => {}} />);
    expect(screen.getByLabelText("reorder a")).not.toBeDisabled();
    expect(screen.getByLabelText("reorder b")).not.toBeDisabled();
  });

  it("reordering visible series does not forget a series absent from this run", () => {
    // "b" is named in the stored order but is not in this run's `names`. The rule the ▲▼ swap kept
    // has to survive the move: it must still be there afterwards.
    const next = reorderSeriesOrder(["a", "b", "c"], ["a", "c"], "c", "a")!;
    expect(next).toContain("b");
    expect(next.indexOf("c")).toBeLessThan(next.indexOf("a"));
  });
});

describe("SeriesListSection — one row per series", () => {
  const base = { names: ["a", "b"], defaultColor: () => "#000000" };

  // Colour and rename used to live in a SECOND list under the same heading, so two series filled
  // four rows and each name was printed twice.
  it("carries colour, rename, order and visibility on the same row", () => {
    const onChange = vi.fn();
    render(<SeriesListSection {...base} onChange={onChange} />);
    for (const label of ["reorder a", "color a", "label a", "Show a"]) {
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
  // swatch therefore stopped belonging to the name beside it as soon as anything was moved.
  //
  // The default colour is now asked for BY NAME, so a reorder cannot move it at all — where the
  // index form only kept them aligned as long as this component and its caller agreed about what an
  // index meant. The stub keys on the name for that reason: an index-keyed stub would pass here
  // whichever way the component asked.
  it("keeps a reordered series' default colour tied to the series, not to its row", () => {
    const palette: Record<string, string> = { a: "#111111", b: "#222222", c: "#333333" };
    render(
      <SeriesListSection
        names={["a", "b", "c"]} order={["c", "a", "b"]}
        defaultColor={(name) => palette[name]} onChange={() => {}}
      />,
    );
    const rows = screen.getAllByRole("switch").map((el) => el.getAttribute("aria-label"));
    expect(rows).toEqual(["Show c", "Show a", "Show b"]);
    expect((screen.getByLabelText("color c") as HTMLInputElement).value).toBe("#333333");
    expect((screen.getByLabelText("color a") as HTMLInputElement).value).toBe("#111111");
  });
});

describe("SeriesListSection — the combo display select", () => {
  const base = { names: ["rev", "cnt"], defaultColor: () => "#000000" };

  // It used to be a THIRD parallel list keyed by metric, printing every series name again beside
  // the row that already carried its colour and its rename.
  it("sits on the series' own row, defaulting to line then bar", () => {
    render(<SeriesListSection {...base} defaultDisplay={(i) => (i === 0 ? "line" : "bar")} onChange={() => {}} />);
    expect(screen.getByLabelText("display rev")).toHaveTextContent("Line");
    expect(screen.getByLabelText("display cnt")).toHaveTextContent("Bar");
  });

  it("writes the chosen display against the SERIES name", () => {
    const onChange = vi.fn();
    render(
      <SeriesListSection {...base} types={{ cnt: "area" }}
        defaultDisplay={(i) => (i === 0 ? "line" : "bar")} onChange={onChange} />,
    );
    expect(screen.getByLabelText("display cnt")).toHaveTextContent("Area");
  });

  // Every other kind draws what its type says, so there is nothing to choose.
  it("is absent when the chart kind offers no per-series display", () => {
    render(<SeriesListSection {...base} onChange={() => {}} />);
    expect(screen.queryByLabelText("display rev")).toBeNull();
  });
});

/**
 * THE DRILL-IN. Per-series line appearance used to be a separate N-row grid of Selects under the
 * same "Series" heading, hidden behind "More options" because it grew without bound — one row per
 * series, and every extra field another Select on every row. It is now one button per row opening
 * a popover that configures ONE series, which is Metabase's shape (`ChartSettingsWidgetPopover`
 * off the series row's ellipsis) and is bounded by construction: a fourth and fifth setting costs
 * a row inside a panel that opens on demand, not a column across every row on screen.
 */
describe("SeriesListSection — the per-series drill-in", () => {
  const base = { names: ["a", "b"], defaultColor: () => "#000000" };

  // A popover holding nothing is a button that opens nothing. Bar, row and scatter have no
  // per-series line appearance at all, and neither do the funnel's steps.
  it("offers no settings button where no series has per-series settings", () => {
    render(<SeriesListSection {...base} onChange={() => {}} />);
    expect(screen.queryByLabelText("settings a")).toBeNull();
  });

  it("puts a settings button on each row that has settings to drill into", () => {
    render(<SeriesListSection {...base} hasLineSettings={() => true} onChange={() => {}} />);
    expect(screen.getByLabelText("settings a")).toBeInTheDocument();
    expect(screen.getByLabelText("settings b")).toBeInTheDocument();
  });

  // The whole point of the drill-in: the controls are NOT on screen until asked for.
  it("reveals that series' four line controls only once its button is clicked", async () => {
    const user = userEvent.setup();
    render(<SeriesListSection {...base} hasLineSettings={() => true} onChange={() => {}} />);
    expect(screen.queryByLabelText("shape a")).toBeNull();
    await user.click(screen.getByLabelText("settings a"));
    for (const f of ["shape", "style", "size", "markers"]) {
      expect(await screen.findByLabelText(`${f} a`)).toBeInTheDocument();
    }
    // ONE series' settings, not the list's. Opening a's popover must not surface b's controls —
    // that is the difference between a drill-in and the grid it replaces.
    expect(screen.queryByLabelText("shape b")).toBeNull();
  });

  // The row a popover was opened from is the only thing that says which series it configures, and
  // the map is keyed by name — a popover that emits under the wrong key silently restyles a
  // different line.
  it("emits a seriesLine patch under the row's own series, merged over the rest of the map", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesListSection
        {...base} hasLineSettings={() => true}
        seriesLine={{ a: { size: "L" } }}
        onChange={onChange}
      />,
    );
    await user.click(screen.getByLabelText("settings b"));
    await user.click(await screen.findByLabelText("shape b"));
    await user.click(await screen.findByRole("option", { name: "Step" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { size: "L" }, b: { shape: "step" } } });
  });

  /**
   * A RENAMED series still emits under its RAW name. `seriesLine` is keyed by the raw series
   * value, the same key space as `seriesColors` and `seriesTypes`, and the header of the popover
   * shows the rename — so it would be easy, and wrong, to thread the displayed string through to
   * the section. That is the drill-on-display-strings mistake this codebase has now made in four
   * chart families: the setting would be stored under a key the builder never looks up, and would
   * silently stop applying the moment anyone typed in the name box.
   *
   * Added because a mutation survived: `name={shown}` in SeriesSettingsPopover left every test
   * green, since none of them opened a popover on a series that had been renamed.
   */
  it("emits under the raw series name even when the series has been renamed", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesListSection {...base} labels={{ b: "Renamed B" }} hasLineSettings={() => true} onChange={onChange} />,
    );
    await user.click(screen.getByLabelText("settings b"));
    await user.click(await screen.findByLabelText("size b"));
    await user.click(await screen.findByRole("option", { name: "S" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { b: { size: "S" } } });
  });

  // Metabase's own popover self-prunes: a series whose resolved display is `bar` shows no line
  // rows at all (measured on the live instance by reading the `hidden` attribute). Here the whole
  // popover is line settings, so pruning removes the button — asked of the caller, which resolves
  // the display through the BUILDER's `seriesDisplayFor` rather than restating the rule.
  it("prunes the button on a series the builder draws as a bar", () => {
    render(
      <SeriesListSection
        {...base} types={{ b: "bar" }} defaultDisplay={() => "line"}
        hasLineSettings={(name) => name !== "b"}
        onChange={() => {}}
      />,
    );
    expect(screen.getByLabelText("settings a")).toBeInTheDocument();
    expect(screen.queryByLabelText("settings b")).toBeNull();
  });
});

/**
 * The list is one row per series and a breakout decides how many series there are, so it is
 * unbounded by construction — a breakout on a 40-value column puts 40 rows in a panel about 300px
 * wide, and everything below the Series group goes off the bottom of the tab. Metabase caps its
 * own at ten (`lib/settings/graph.ts`, `truncateAfter: 10`) behind a "more" affordance; so does
 * this. It is the piece that actually answers "unreadable at ten series", and it is worth having
 * whether or not the drill-in exists.
 */
describe("SeriesListSection — a long list truncates", () => {
  const many = Array.from({ length: 13 }, (_, i) => `s${i}`);
  const rows = () => screen.getAllByRole("switch").map((el) => el.getAttribute("aria-label"));

  it("renders ten rows and an affordance naming how many are left", () => {
    render(<SeriesListSection names={many} defaultColor={() => "#000000"} onChange={() => {}} />);
    expect(rows()).toHaveLength(10);
    expect(screen.getByRole("button", { name: "Show 3 more series" })).toBeInTheDocument();
  });

  // Which ten: the first ten in the RANKED order, not the first ten names. A truncation that
  // ignored the stored order would hide whichever series the user had just moved to the top.
  it("keeps the ranked order across the cut", () => {
    render(
      <SeriesListSection names={many} order={["s12", "s11"]} defaultColor={() => "#000000"} onChange={() => {}} />,
    );
    expect(rows()[0]).toBe("Show s12");
    expect(rows()[1]).toBe("Show s11");
    expect(rows()).not.toContain("Show s9");
  });

  it("reveals the rest when the affordance is activated", async () => {
    const user = userEvent.setup();
    render(<SeriesListSection names={many} defaultColor={() => "#000000"} onChange={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Show 3 more series" }));
    expect(rows()).toHaveLength(13);
    expect(screen.getByRole("button", { name: "Show fewer series" })).toBeInTheDocument();
  });

  // Boundary. Ten is not "more than ten", and a list capped at exactly its own length would show
  // a "Show 0 more series" button that does nothing. (This case passes against the pre-truncation
  // code too — it earns its place by dying under an off-by-one mutation, not by failing first.)
  it("leaves a list of exactly ten alone", () => {
    render(<SeriesListSection names={many.slice(0, 10)} defaultColor={() => "#000000"} onChange={() => {}} />);
    expect(rows()).toHaveLength(10);
    expect(screen.queryByRole("button", { name: /more series/ })).toBeNull();
  });
});
