import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import type { PieConfig } from "@/api/charts";
import { reorderSeriesOrder } from "../SeriesListSection";
import { PieDisplaySection } from "../PieDisplaySection";
import { buildPieOption, DEFAULT_SLICE_THRESHOLD } from "@/components/chart/echarts/options/pie";

/** The swatch is a BUTTON with a background now, not an `<input type="color">` with a value — the
 *  palette picker replaced the OS colour dialog. jsdom hands `style.backgroundColor` back as
 *  `rgb(r, g, b)`, so it is converted here rather than rewriting every assertion below: what these
 *  tests are about is WHICH COLOUR the panel shows, and hex is how the palette states it. */
function hexOf(el: HTMLElement): string {
  const m = /rgb\((\d+),\s*(\d+),\s*(\d+)\)/.exec(el.style.backgroundColor || "");
  if (!m) return (el.style.backgroundColor || "").toLowerCase();
  return "#" + [1, 2, 3].map((i) => Number(m[i]).toString(16).padStart(2, "0")).join("");
}

describe("PieDisplaySection", () => {
  // Legend and Show labels both default from buildPieOption at render time (a pie's colours
  // and slices are unreadable without them), so they are a three-way Auto/On/Off select
  // rather than a Switch — `undefined` is a real, visible state, not "off". A two-state
  // Switch bound to `!!config.showLegend` would show OFF while the chart draws ON, the same
  // "switch that lies" bug already fixed for chart 01's legend/compact-numbers and chart 02's
  // connect-gaps. Each interaction starts from a value that differs from the target, since a
  // Radix Select does not fire onValueChange when the already-selected item is re-clicked.
  describe("Legend (Auto/On/Off)", () => {
    it("choosing On emits true", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "On" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: true });
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ showLegend: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Legend"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showLegend: undefined });
    });

    it("shows Auto selected when the config leaves it unset", () => {
      render(<PieDisplaySection config={{}} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Legend")).getByText("Auto")).toBeInTheDocument();
    });
  });

  describe("Show labels (Auto/On/Off)", () => {
    it("choosing On emits true", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "On" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: true });
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ showLabels: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show labels"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showLabels: undefined });
    });

    it("shows Auto selected when the config leaves it unset", () => {
      render(<PieDisplaySection config={{}} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Show labels")).getByText("Auto")).toBeInTheDocument();
    });
  });

  // `showTotal` now defaults ON for a multi-ring pie (buildPieOption resolves it at render
  // time), so a two-state Switch bound to `!!config.showTotal` would read OFF while the chart
  // draws the total — the same "switch that lies" this section already fixed for Legend and
  // Show labels. It also has to be REACHABLE on a multi-ring pie, which need not have `donut`
  // set at all: the hole is there either way, and hiding the control behind `donut` would leave
  // a newly-on-by-default total with no way to turn it off.
  describe("Show total (Auto/On/Off)", () => {
    it("is offered for a multi-ring pie even with donut unset", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Show total")).toBeInTheDocument();
    });

    it("is still offered for a single-ring donut", () => {
      render(<PieDisplaySection config={{ donut: true, dimensions: ["a"] }} onChange={() => {}} />);
      expect(screen.getByLabelText("Show total")).toBeInTheDocument();
    });

    it("is not offered for a single-ring pie with no hole to write in", () => {
      render(<PieDisplaySection config={{ dimensions: ["a"] }} onChange={() => {}} />);
      expect(screen.queryByLabelText("Show total")).not.toBeInTheDocument();
    });

    it("shows Auto when nothing is stored, rather than claiming Off", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} />);
      expect(within(screen.getByLabelText("Show total")).getByText("Auto")).toBeInTheDocument();
    });

    it("choosing Off emits false", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show total"));
      await user.click(await screen.findByRole("option", { name: "Off" }));
      expect(onChange).toHaveBeenCalledWith({ showTotal: false });
    });

    it("choosing Auto emits undefined", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ dimensions: ["a", "b"], showTotal: true }} onChange={onChange} />);
      await user.click(screen.getByLabelText("Show total"));
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ showTotal: undefined });
    });
  });

  it("per-slice colour swatch patches sliceColors by (raw) name", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{}} onChange={onChange} slices={["A", "B"]} />);
    // Through the picker the way a user reaches it: the swatch opens a popover of the chart
    // palette, and each swatch is labelled by its hex. `#ff0000` is not in that palette — the
    // custom input inside the popover is the route to a colour like that, and it is the second
    // row rather than the first, which is the point of the change.
    fireEvent.click(screen.getByLabelText("color B"));
    fireEvent.click(screen.getByLabelText("#EF8C8C"));
    expect(onChange).toHaveBeenCalledWith({ sliceColors: { B: "#EF8C8C" } });
  });

  it("per-slice rename patches sliceNames and clears (deletes the key) when emptied", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ sliceNames: { A: "Old" } }} onChange={onChange} slices={["A"]} />);
    fireEvent.change(screen.getByLabelText("label A"), { target: { value: "Alpha" } });
    expect(onChange).toHaveBeenCalledWith({ sliceNames: { A: "Alpha" } });
    fireEvent.change(screen.getByLabelText("label A"), { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith({ sliceNames: {} });
  });

  it("toggling a slice's switch off emits hiddenSlices containing it", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{}} onChange={onChange} slices={["A", "B"]} />);
    fireEvent.click(screen.getByLabelText("Show B"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSlices: ["B"] });
  });

  it("toggling a hidden slice's switch back on removes it from hiddenSlices", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ hiddenSlices: ["B"] }} onChange={onChange} slices={["A", "B"]} />);
    fireEvent.click(screen.getByLabelText("Show B"));
    expect(onChange).toHaveBeenCalledWith({ hiddenSlices: [] });
  });

  it("ranks rows by the stored sliceOrder, keeping unranked slices after it in their natural place", () => {
    render(<PieDisplaySection config={{ sliceOrder: ["C", "A"] }} onChange={() => {}} slices={["A", "B", "C"]} />);
    // Filter to the per-slice switches: the section has other Switches too (Legend, Donut,
    // Show labels…) whose aria-labels don't follow the "Show <slice>" shape.
    const rows = screen.getAllByRole("switch")
      .map((el) => el.getAttribute("aria-label"))
      .filter((l): l is string => !!l && ["Show A", "Show B", "Show C"].includes(l));
    expect(rows).toEqual(["Show C", "Show A", "Show B"]);
  });

  /**
   * Driven through the pure helper, not the DOM: reordering is a drag now and no test in this repo
   * drives a dnd drop (dnd-kit's keyboard sensor reads element rects, and jsdom has none). The pie
   * shares `reorderSeriesOrder` with the series list because it shares the rule.
   */
  it("moving a slice writes the order the pie will draw", () => {
    expect(reorderSeriesOrder(undefined, ["A", "B", "C"], "C", "B")).toEqual(["A", "C", "B"]);
  });

  it("merges into the stored order rather than replacing it", () => {
    // "B" is named in the stored order but is not present in this run's slices — it must survive.
    const next = reorderSeriesOrder(["A", "B", "C"], ["A", "C"], "C", "A")!;
    expect(next).toContain("B");
    expect(next.indexOf("C")).toBeLessThan(next.indexOf("A"));
  });

  it("gives every row a grip, and no row a disabled one — a drag has no ends to hit", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={["A", "B"]} />);
    expect(screen.getByLabelText("reorder A")).not.toBeDisabled();
    expect(screen.getByLabelText("reorder B")).not.toBeDisabled();
  });

  it("the slice rows do not render when there are no slices", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
    expect(screen.queryByText("Slices")).not.toBeInTheDocument();
  });

  // Was "hidden until a percent visibility is chosen". The default is now "legend" at any ring
  // count (task 5), so with nothing stored the chart IS drawing percentages and the box that sets
  // their precision belongs on screen. The cascade survives, on an explicit Off — see
  // "still hides Decimal places when Percentages is explicitly Off" below.
  it("Decimal places is offered with nothing stored, because Auto draws percentages", () => {
    render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
    expect(screen.getByLabelText("Decimal places")).toBeInTheDocument();
  });

  // `legend` completes Metabase's own `pie.percent_visibility` domain — it is the value its
  // sample sunburst is configured with, and the one egret was missing.
  describe("Percentages", () => {
    const openPercentages = async (user: ReturnType<typeof userEvent.setup>) => {
      await user.click(screen.getByLabelText("Percentages"));
    };

    it("offers In legend alongside the values that were already there", async () => {
      const user = userEvent.setup();
      render(<PieDisplaySection config={{}} onChange={() => {}} slices={[]} />);
      await openPercentages(user);
      const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
      expect(options).toEqual(["Auto", "Off", "In legend", "In chart", "Both"]);
    });

    it("choosing In legend emits the legend value", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{}} onChange={onChange} slices={[]} />);
      await openPercentages(user);
      await user.click(await screen.findByRole("option", { name: "In legend" }));
      expect(onChange).toHaveBeenCalledWith({ percentVisibility: "legend" });
    });

    it("choosing Auto clears the stored value", async () => {
      const user = userEvent.setup();
      const onChange = vi.fn();
      render(<PieDisplaySection config={{ percentVisibility: "both" }} onChange={onChange} slices={[]} />);
      await openPercentages(user);
      await user.click(await screen.findByRole("option", { name: "Auto" }));
      expect(onChange).toHaveBeenCalledWith({ percentVisibility: undefined });
    });

    // The control has to show what the chart will draw. With nothing stored, a multi-ring pie
    // draws its percentages in the legend, so a select without Auto would read "Off" while
    // the legend carried numbers.
    it("shows Auto when nothing is stored", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} slices={[]} />);
      expect(within(screen.getByLabelText("Percentages")).getByText("Auto")).toBeInTheDocument();
    });

    it("offers Decimal places under Auto on a multi-ring pie, since Auto draws percentages there", () => {
      render(<PieDisplaySection config={{ dimensions: ["a", "b"] }} onChange={() => {}} slices={[]} />);
      expect(screen.getByLabelText("Decimal places")).toBeInTheDocument();
    });

    // TASK 5. Metabase's `getDefaultPercentVisibility` is `() => "legend"` — no ring count in it —
    // so Auto now draws percentages on a one-ring pie too, and the box that sets their precision
    // has to be reachable without first turning a setting on that is already on.
    it("offers Decimal places under Auto on a single-ring pie, which now draws them in the legend", () => {
      render(<PieDisplaySection config={{ dimensions: ["a"] }} onChange={() => {}} slices={[]} />);
      expect(screen.getByLabelText("Decimal places")).toBeInTheDocument();
    });

    // THE CASCADE ITSELF IS KEPT, and it is Metabase's own: `pie.decimal_places.getHidden` reads
    // `settings["pie.percent_visibility"] == null || === "off"`. Now that the default is never
    // "off" it can only bite on an EXPLICIT Off, where the box would be setting the precision of a
    // number nothing draws — a control that lies, which is what this PR exists to remove.
    it("still hides Decimal places when Percentages is explicitly Off", () => {
      render(<PieDisplaySection config={{ dimensions: ["a"], percentVisibility: "off" }} onChange={() => {}} slices={[]} />);
      expect(screen.queryByLabelText("Decimal places")).not.toBeInTheDocument();
    });

    // Whatever the chart does with an unrecognised stored value, the control must say the same
    // thing — and the chart draws nothing.
    it("reports an unknown stored value as Off, matching what the chart draws", () => {
      render(
        <PieDisplaySection
          config={{ dimensions: ["a", "b"], percentVisibility: "everywhere" as never }}
          onChange={() => {}} slices={[]} />,
      );
      expect(within(screen.getByLabelText("Percentages")).getByText("Off")).toBeInTheDocument();
    });
  });

  it("Decimal places patches decimals as a whole number", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ percentVisibility: "inside" }} onChange={onChange} slices={[]} />);
    fireEvent.change(screen.getByLabelText("Decimal places"), { target: { value: "0" } });
    expect(onChange).toHaveBeenCalledWith({ decimals: 0 });
  });

  // The chart derives its own precision from the shares it draws once nothing is stored, so a
  // number in this box with nothing behind it would be a control claiming a value the chart isn't
  // using — the fault this panel already fixed for the legend, the labels and the centre total.
  it("Decimal places shows Auto, not a number, when nothing is stored", () => {
    render(<PieDisplaySection config={{ percentVisibility: "inside" }} onChange={() => {}} slices={[]} />);
    const input = screen.getByLabelText("Decimal places") as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("Auto");
  });

  it("Decimal places shows a stored precision, and clearing it goes back to Auto", () => {
    const onChange = vi.fn();
    render(<PieDisplaySection config={{ percentVisibility: "inside", decimals: 3 }} onChange={onChange} slices={[]} />);
    expect((screen.getByLabelText("Decimal places") as HTMLInputElement).value).toBe("3");
    fireEvent.change(screen.getByLabelText("Decimal places"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ decimals: undefined });
  });
});

/**
 * "GROUP SLICES BELOW" KEEPS THE NUMBER BEING TYPED INTO IT.
 *
 * These drive the box with real key presses through a STATEFUL parent, because the defect lives
 * entirely in the round trip: the handler stores a number, the parent feeds it back as the
 * controlled `value`, and the re-render overwrites what is in the box. A `fireEvent.change` with
 * the final value, or a spy parent that never feeds anything back, closes that loop and passes
 * against the broken control.
 *
 * WHAT THE BROKEN CONTROL DID, measured in Chrome (playwright, `type=number` + the old handler,
 * driven by keyboard.press and reading `input.value` after each key):
 *
 *   typing 4 . 5 over a selected "2.5"  →  [4 → box "4", stored 4]
 *                                          [. → box "0", stored 0]   ← collapsed
 *                                          [5 → box "5", stored 5]   ← meant 4.5, got 5
 *   clearing with Backspace             →  box "0", stored 0
 *
 * The cause of both is one line: a `type=number` input whose contents are not a valid number
 * reports `value === ""` (`validity.badInput === true`) — the browser does this both for an empty
 * box and for the instant "4." exists — and `Number("")` is 0. 0 is not "unset" here; it is the
 * setting that turns the "Other" fold OFF entirely (charts.ts: "0 = off"), so the control silently
 * stored a different setting and, showing "0", offered no way back to the 2.5 default.
 *
 * JSDOM CANNOT SEE THE COLLAPSE THE SAME WAY and these tests do not pretend it can. user-event
 * buffers the "." itself rather than letting the number input go bad, so no empty-string change
 * event is ever delivered mid-decimal. What jsdom shows instead is the same defect's other face:
 * the "." never reaches the box at all, which is why the display assertion below (`"4."`) is the
 * one that fails on the old control, not the stored value. The empty-string change event that
 * Chrome delivers mid-decimal is exercised directly by the clear tests.
 */
describe("PieDisplaySection — Group slices below", () => {
  /** A parent that actually applies the patch, so the control is fed its own output back. */
  function Harness({ initial = {} }: { initial?: PieConfig }) {
    const [config, setConfig] = useState<PieConfig>(initial);
    return (
      <PieDisplaySection
        config={config}
        onChange={(patch) => setConfig((c) => ({ ...c, ...patch }))}
        slices={[]}
      />
    );
  }
  const box = () => screen.getByLabelText("Group slices below") as HTMLInputElement;
  /** What the panel is currently storing, read back off the control that displays it. */
  const stored = () => box().value;

  it("shows the default the chart actually folds at", () => {
    render(<Harness />);
    expect(box().value).toBe(String(DEFAULT_SLICE_THRESHOLD));
  });

  // The typed characters must survive in the box while a decimal is half-written. On the old
  // control the "." is simply not there afterwards (jsdom), and in Chrome it took the whole
  // field to "0" — the two faces of the same missing draft state.
  it("keeps a half-typed decimal in the box instead of swallowing it", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.clear(box());
    await user.type(box(), "4");
    expect(box().value).toBe("4");
    await user.type(box(), ".");
    expect(box().value).toBe("4.");
    await user.type(box(), "5");
    expect(box().value).toBe("4.5");
  });

  // …and the number behind it never passes through 0 on the way. 0 turns the fold off, so a
  // single frame of it is a real, if brief, different chart.
  it("never stores 0 while a decimal is being typed", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [config, setConfig] = useState<PieConfig>({});
      return (
        <PieDisplaySection
          config={config}
          onChange={(patch) => {
            if ("sliceThreshold" in patch) seen.push(patch.sliceThreshold);
            setConfig((c) => ({ ...c, ...patch }));
          }}
          slices={[]}
        />
      );
    }
    render(<Spy />);
    await user.clear(box());
    seen.length = 0; // the clear itself is asserted below; this is about the digits after it
    await user.type(box(), "4.5");
    expect(seen).not.toContain(0);
    expect(seen.at(-1)).toBe(4.5);
  });

  // Clearing is "no threshold of my own", i.e. back to the default — NOT 0, which is the
  // distinct "never fold anything" setting. This is also the exact change event Chrome delivers
  // mid-decimal, so it pins that path too.
  it("clearing the box means unset, not the fold-nothing 0", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [config, setConfig] = useState<PieConfig>({ sliceThreshold: 10 });
      return (
        <PieDisplaySection
          config={config}
          onChange={(patch) => {
            if ("sliceThreshold" in patch) seen.push(patch.sliceThreshold);
            setConfig((c) => ({ ...c, ...patch }));
          }}
          slices={[]}
        />
      );
    }
    render(<Spy />);
    expect(box().value).toBe("10");
    await user.clear(box());
    expect(seen).toEqual([undefined]);
    expect(box().value).not.toBe("0");
  });

  // The default has to be REACHABLE again — clearing was the only route back to it, and it led
  // to 0 instead, permanently.
  it("clearing gets the default back rather than stranding the panel on 0", async () => {
    const user = userEvent.setup();
    render(<Harness initial={{ sliceThreshold: 10 }} />);
    await user.clear(box());
    await user.tab(); // leaving the field settles it on what the chart will use
    expect(box().value).toBe(String(DEFAULT_SLICE_THRESHOLD));
  });

  // 0 is still typeable, and still means what it means: fold nothing. It has to travel through
  // a stateful parent like the rest — against a spy that never applies the patch the box keeps
  // redisplaying the old 10, so the "0" lands on the end of it and the test measures the stub.
  it("still lets 0 be chosen deliberately", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [config, setConfig] = useState<PieConfig>({ sliceThreshold: 10 });
      return (
        <PieDisplaySection
          config={config}
          onChange={(patch) => {
            if ("sliceThreshold" in patch) seen.push(patch.sliceThreshold);
            setConfig((c) => ({ ...c, ...patch }));
          }}
          slices={[]}
        />
      );
    }
    render(<Spy />);
    await user.clear(box());
    await user.type(box(), "0");
    expect(seen.at(-1)).toBe(0);
    expect(box().value).toBe("0");
  });

  // Text that is not a number must not be committed as one. `Number("4.5.6")` is NaN, and a NaN
  // threshold makes every `share < threshold` comparison false — the fold would quietly stop
  // happening rather than the keystroke being ignored.
  it("does not commit text that is not a number", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [config, setConfig] = useState<PieConfig>({ sliceThreshold: 10 });
      return (
        <PieDisplaySection
          config={config}
          onChange={(patch) => {
            if ("sliceThreshold" in patch) seen.push(patch.sliceThreshold);
            setConfig((c) => ({ ...c, ...patch }));
          }}
          slices={[]}
        />
      );
    }
    render(<Spy />);
    await user.type(box(), "x");
    expect(seen).toEqual([]);
    expect(box().value).toBe("10x"); // the keystroke is still visible, just not stored
    await user.tab();
    expect(box().value).toBe("10"); // …and leaving the field puts back what the chart is using
  });

  it("stores a whole number typed over the default", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.clear(box());
    await user.type(box(), "12");
    expect(stored()).toBe("12");
  });
});

/**
 * THE SWATCH IS THE COLOUR THE PIE DRAWS.
 *
 * The default was `SERIES_PALETTE[i % length]` — a by-POSITION rule the renderer stopped following
 * when ring 0 moved to `colorsForKeys`. So these tests BUILD THE PIE and read ring 0's colours back,
 * rather than comparing the panel to `colorsForKeys` (which would prove wiring, not agreement).
 */
describe("PieDisplaySection — the slice swatch and the slice the chart draws", () => {
  const cols = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];
  /**
   * Value-desc, which is ring 0's own emission order, and deliberately NOT alphabetical: on the hash
   * path `colorsForKeys` assigns in sorted key order and so returns its Record in sorted order, and
   * a pre-sorted fixture makes a by-name lookup and a by-position one hand back the same sequence.
   * "Gadget" and "Widget" also hash onto one slot, so this walks the collision path too.
   */
  const rows: unknown[][] = [["Widget", 40], ["Doohickey", 30], ["Gizmo", 20], ["Gadget", 10]];
  /** What `PieDisplayControl` (viz/simple.tsx) hands the panel: the distinct raw values of the first
   *  dimension, in row order. */
  const SLICES = ["Widget", "Doohickey", "Gizmo", "Gadget"];

  /** Ring 0's colours, keyed by slice name. */
  const pieColors = (cfg: Record<string, unknown> = {}): Record<string, string> => {
    const opt = buildPieOption(cols, rows, { dimensions: ["category"], metric: "m", ...cfg }) as unknown as {
      series: { data: { name: string; itemStyle: { color: string } }[] }[];
    };
    return Object.fromEntries(opt.series[0].data.map((d) => [d.name, d.itemStyle.color.toLowerCase()]));
  };
  /** A native `<input type="color">` normalises its value to lower-case (HTML spec), which is also
   *  what the panel writes back — so both sides are compared in that casing. */
  const swatch = (name: string) => hexOf(screen.getByLabelText(`color ${name}`));

  it("shows each slice the colour the pie draws it in, not the palette slot of its row", () => {
    const chart = pieColors();
    expect(Object.keys(chart)).toEqual(SLICES); // the emission order this pins against
    render(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={SLICES} />);
    for (const name of SLICES) expect(swatch(name)).toBe(chart[name]);
  });

  it("keeps a slice's swatch when the rows are reordered", () => {
    const { rerender } = render(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={SLICES} />);
    const before = Object.fromEntries(SLICES.map((n) => [n, swatch(n)]));
    rerender(<PieDisplaySection config={{ dimensions: ["category"] }} onChange={() => {}} slices={[...SLICES].reverse()} />);
    for (const name of SLICES) expect(swatch(name)).toBe(before[name]);
  });

  /** `sliceColors` reaches ring 0 as `colorsForKeys`' `existing` map, where an entry CONSUMES its
   *  colour — so a pin moves whichever slice would have hashed onto it, in the chart and here. */
  it("folds a pinned colour in, so the slice it displaced moves here too", () => {
    const sliceColors = { Gadget: "#98d9d9" }; // SERIES_PALETTE[6], lower-cased as the picker gives it back
    const chart = pieColors({ sliceColors });
    render(<PieDisplaySection config={{ dimensions: ["category"], sliceColors }} onChange={() => {}} slices={SLICES} />);
    for (const name of SLICES) expect(swatch(name)).toBe(chart[name]);
    expect(new Set(SLICES.map(swatch)).size).toBe(4); // nobody was handed the pinned ink twice
  });

  /**
   * A hidden slice's rows are dropped before the ring is built (buildPieRings), so it is not a key
   * and spends no colour: the slices that remain must show what the chart now draws them in.
   *
   * GADGET IS THE ONE TO HIDE, not Widget. The two collide on one palette slot, and "Gadget" sorts
   * first and takes it while "Widget" walks along — so hiding Gadget frees that slot and MOVES
   * Widget, which is the only way this fixture can tell a re-keyed panel from one that quietly kept
   * every name. Hiding Widget instead leaves the other three exactly where they were, and this test
   * passed with the filter deleted.
   */
  it("re-keys on the slices that survive a hide, as the chart does", () => {
    const chart = pieColors({ hiddenSlices: ["Gadget"] });
    expect(Object.keys(chart)).not.toContain("Gadget");
    expect(chart["Widget"]).not.toBe(pieColors()["Widget"]); // the premise: hiding Gadget moved it
    render(<PieDisplaySection config={{ dimensions: ["category"], hiddenSlices: ["Gadget"] }} onChange={() => {}} slices={SLICES} />);
    for (const name of Object.keys(chart)) expect(swatch(name)).toBe(chart[name]);
    // …and the hidden row still shows something: what it will be given when it is turned back on.
    expect(swatch("Gadget")).toBe(pieColors()["Gadget"]);
  });
});
