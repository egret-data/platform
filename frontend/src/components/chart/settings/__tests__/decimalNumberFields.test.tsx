import { useState } from "react";
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { CartesianConfig, WaterfallConfig, BoxplotConfig } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { ChartAxesSection, type AxesConfig } from "../ChartAxesSection";
import { AxisRangeOptions, GoalOptions } from "../CartesianDisplaySection";
import { WaterfallFormatSection } from "../WaterfallDisplaySection";
import { BoxplotDisplaySection } from "../BoxplotDisplaySection";
import { SegmentRangesSection, type SegmentRow } from "../SegmentRangesSection";
import { FormatSection } from "../FormatSection";
import { DecimalInput } from "../DecimalInput";
import { fieldDescriptor } from "@/components/chart/viz/fields";

/**
 * EVERY SETTINGS BOX A FRACTION CAN LEGITIMATELY BE TYPED INTO, ASSERTED THE SAME WAY.
 *
 * THE DEFECT. `<input type="number">` reports `value === ""` with `validity.badInput === true`
 * for as long as its contents are not a valid number — and "4." is not a valid number. So the
 * instant the decimal point is pressed the browser fires an `input` event carrying `""`, which
 * every handler in these files read as either `Number("") === 0` or as "the user cleared this".
 * Neither is what happened.
 *
 * Measured in Chrome 150 with real key presses (playwright `keyboard.press`, NOT `fill()`), on a
 * bare `<input type="number">` and on `<input type="number" step="1" min="2" max="20">` alike:
 *
 *   press "4"  ->  value "4"    badInput false
 *   press "."  ->  value ""     badInput TRUE      <- the keystroke the field cannot hold
 *   press "5"  ->  value "4.5"  badInput false
 *
 * `step` does not gate the character: it only decides `validity.stepMismatch`, which surfaces at
 * form submission and never blocks typing. So "an integer-shaped number input can't receive a dot"
 * is false in Chrome. Integer fields are still out of scope here, but on the stated rule — a `.` is
 * not a legitimate keystroke in "Number of tick marks" — and NOT because the browser refuses it.
 *
 * WHAT JSDOM CAN AND CANNOT SEE. `user-event` buffers the "." itself rather than letting the number
 * input go bad, so the empty-string change event Chrome delivers mid-decimal is never delivered
 * here and these tests do not pretend otherwise. What jsdom shows is the SAME LOSS BY THE OTHER
 * ROUTE: a "." typed as its own keystroke is dropped on the floor and never applied, so the user
 * who pressed 4 . 5 ends up with FORTY-FIVE. Same input, same wrong number, different mechanism.
 * That is why every case below types the three characters as THREE SEPARATE `user.type` calls —
 * one call with "4.5" in it would let user-event's buffer paper over the drop and pass against the
 * broken control, which is exactly the shape of blindness this file exists to avoid.
 *
 * The Chrome-only half — the empty-string event — has no jsdom expression at all. The only thing
 * that can be pinned for it is its cause, so each case also asserts the field is not a
 * `type="number"` in the first place.
 *
 * Every harness applies its own patch. A `vi.fn()` parent never re-renders the box with the new
 * value, so the control keeps redisplaying the old one and the assertions measure the stub rather
 * than the product.
 *
 * WHAT WAS MEASURED TO BE BLIND, against the broken `type="number"` Goal value and no other change
 * — all four of these PASSED on the defect, which is why none of them is used below:
 *
 *   A  `await user.type(box, "4.5")` in one call, asserting only the final "4.5"
 *   B  `fireEvent.change(box, { target: { value: "4.5" } })`, asserting the final value
 *   C  typing "42" — a value that round-trips through `Number()` unchanged, so it cannot move
 *   D  five SEPARATE calls typing 0 . 0 0 1, asserting only the final "0.001" — user-event's
 *      number-input buffer lives on the element and survives across calls, so it replays the
 *      whole string in the end even though every intermediate frame was wrong
 *
 * The one thing the broken control cannot do is DISPLAY THE INTERMEDIATE, so every case here
 * asserts the box after each keystroke rather than at the end of the word.
 */

/** Type 4, then . , then 5 as three separate keystroke batches, checking the box after each. */
async function typesAHalfWrittenDecimal(
  user: ReturnType<typeof userEvent.setup>,
  box: () => HTMLInputElement,
) {
  await user.clear(box());
  await user.type(box(), "4");
  expect(box().value).toBe("4");
  await user.type(box(), ".");
  // The whole defect in one assertion: on `type="number"` the "." is simply not here.
  expect(box().value).toBe("4.");
  await user.type(box(), "5");
  expect(box().value).toBe("4.5");
}

/** The cause of the Chrome-only half, pinned where jsdom cannot observe the effect. */
function isNotANumberInput(box: HTMLInputElement) {
  expect(box.getAttribute("type")).toBe("text");
  expect(box.getAttribute("inputmode")).toBe("decimal");
}

const box = (label: string) => () => screen.getByLabelText(label) as HTMLInputElement;

// ---------------------------------------------------------------------------------------------
// Axis bounds — ChartAxesSection (waterfall / box plot / sankey) and CartesianDisplaySection.
// ONE implementation of the pair now (`ValueRangeRows`), rendered by both panels — but both are
// still asserted, because they reach it through different props (the cartesian one names the axis
// after the chart's orientation) and because a shared component is only shared for as long as
// nobody copies it back out. This bug lived in two places once already.
// ---------------------------------------------------------------------------------------------

describe("Y min / Y max — ChartAxesSection", () => {
  /** The pair only renders once the range is not auto, so it starts from a stored bound. */
  function Harness() {
    const [config, setConfig] = useState<AxesConfig>({ yMin: 0, yMax: 100 });
    return <ChartAxesSection config={config} onChange={(p) => setConfig((c) => ({ ...c, ...p }))} />;
  }

  it("keeps a half-written decimal in Y min instead of dropping the point", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Y min"));
  });

  it("keeps a half-written decimal in Y max instead of dropping the point", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Y max"));
  });

  it("is not a number input, which is what delivers the empty-string event in Chrome", () => {
    render(<Harness />);
    isNotANumberInput(box("Y min")());
    isNotANumberInput(box("Y max")());
  });

  // Clearing has to remain "auto", not 0 — a 0 floor is a real, different axis.
  it("clearing Y min leaves the bound unset rather than pinning the axis at 0", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [config, setConfig] = useState<AxesConfig>({ yMin: 5, yMax: 100 });
      return (
        <ChartAxesSection
          config={config}
          onChange={(p) => {
            if ("yMin" in p) seen.push(p.yMin);
            setConfig((c) => ({ ...c, ...p }));
          }}
        />
      );
    }
    render(<Spy />);
    await user.clear(box("Y min")());
    expect(seen).toEqual([undefined]);
    expect(box("Y min")().value).not.toBe("0");
  });
});

describe("value-axis min / max — CartesianDisplaySection", () => {
  /** A stored bound to start from: the pair is gated behind "Auto y-axis range" here too now, so an
   *  unbounded config renders no boxes at all to type into. */
  function Harness() {
    const [config, setConfig] = useState<CartesianConfig>({ yMetrics: ["revenue"], yMin: 0, yMax: 100 });
    return (
      <AxisRangeOptions
        kind="bar"
        config={config}
        onChange={(p) => setConfig((c) => ({ ...c, ...p }))}
      />
    );
  }

  it("keeps a half-written decimal in the min bound", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Y min"));
  });

  it("keeps a half-written decimal in the max bound", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Y max"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("Y min")());
    isNotANumberInput(box("Y max")());
  });
});

// ---------------------------------------------------------------------------------------------
// Goal (reference line) value — three separate copies, one per chart family.
// ---------------------------------------------------------------------------------------------

describe("Goal value — CartesianDisplaySection", () => {
  function Harness() {
    const [config, setConfig] = useState<CartesianConfig>({ yMetrics: ["revenue"] });
    return <GoalOptions kind="bar" config={config} onChange={(p) => setConfig((c) => ({ ...c, ...p }))} />;
  }

  it("keeps a half-written decimal", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Goal value"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("Goal value")());
  });
});

describe("Goal value — WaterfallFormatSection", () => {
  function Harness() {
    const [config, setConfig] = useState<WaterfallConfig>({});
    return <WaterfallFormatSection config={config} onChange={(p) => setConfig((c) => ({ ...c, ...p }))} />;
  }

  it("keeps a half-written decimal", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Goal value"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("Goal value")());
  });
});

describe("Goal value — BoxplotDisplaySection", () => {
  function Harness() {
    const [config, setConfig] = useState<BoxplotConfig>({});
    return <BoxplotDisplaySection config={config} onChange={(p) => setConfig((c) => ({ ...c, ...p }))} />;
  }

  it("keeps a half-written decimal", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Goal value"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("Goal value")());
  });
});

// ---------------------------------------------------------------------------------------------
// Coloured-range bounds, shared by the gauge and the scalar. A band edge is a data value, so a
// fraction is as ordinary here as anywhere.
// ---------------------------------------------------------------------------------------------

describe("range bounds — SegmentRangesSection", () => {
  function Harness({ nullableBounds = false }: { nullableBounds?: boolean }) {
    const [rows, setRows] = useState<SegmentRow[]>([{ min: 0, max: 10, color: "#111111" }]);
    return <SegmentRangesSection rows={rows} onChange={setRows} nullableBounds={nullableBounds} minRows={1} />;
  }

  it("keeps a half-written decimal in a range min", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("range 1 min"));
  });

  it("keeps a half-written decimal in a range max", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("range 1 max"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("range 1 min")());
    isNotANumberInput(box("range 1 max")());
  });

  // The gauge cannot hold a null bound, so an emptied box is REJECTED there rather than cleared —
  // and must not fall through to 0, which is a real, different arc.
  it("emptying a non-nullable bound does not silently store 0", async () => {
    const user = userEvent.setup();
    const seen: SegmentRow[][] = [];
    function Spy() {
      const [rows, setRows] = useState<SegmentRow[]>([{ min: 25, max: 50, color: "#111111" }]);
      return (
        <SegmentRangesSection
          rows={rows}
          minRows={1}
          onChange={(next) => {
            seen.push(next);
            setRows(next);
          }}
        />
      );
    }
    render(<Spy />);
    await user.clear(box("range 1 min")());
    expect(seen).toEqual([]);
  });
});

// ---------------------------------------------------------------------------------------------
// "Multiply by a number" — a column-format scale. Fractions are the POINT of this one: dividing by
// a thousand is `0.001`, and every value below 1 is a decimal that has to survive being typed.
// ---------------------------------------------------------------------------------------------

describe("Multiply by a number — FormatSection", () => {
  function Harness() {
    const [value, setValue] = useState<Record<string, ColumnFormat>>({});
    return <FormatSection columns={[{ name: "revenue", type: "double" }]} value={value} onChange={setValue} />;
  }
  const open = async (user: ReturnType<typeof userEvent.setup>) => {
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: /revenue/ }));
  };

  it("keeps a half-written decimal", async () => {
    const user = userEvent.setup();
    await open(user);
    await typesAHalfWrittenDecimal(user, box("Multiply by a number"));
  });

  // 0.001 is the everyday use (show thousands as units), and its leading "0." is the longest a
  // number input stays blank for.
  //
  // THE FINAL VALUE IS NOT THE ASSERTION HERE, deliberately. Measured against the broken control:
  // typing 0 . 0 0 1 as five separate calls ends on "0.001" anyway, because user-event's own
  // number-input buffer lives on the ELEMENT and survives across calls, so it eventually replays
  // the whole string. A test that only checked the end of this sequence would pass against
  // `type="number"` and prove nothing. What the broken control cannot do is show the intermediate.
  it("can be given a scale below 1, and shows the leading '0.' while it is being typed", async () => {
    const user = userEvent.setup();
    await open(user);
    const b = box("Multiply by a number");
    await user.type(b(), "0");
    await user.type(b(), ".");
    expect(b().value).toBe("0."); // broken: "0" — the point is nowhere
    await user.type(b(), "0");
    expect(b().value).toBe("0.0");
    await user.type(b(), "0");
    await user.type(b(), "1");
    expect(b().value).toBe("0.001");
  });

  it("is not a number input", async () => {
    const user = userEvent.setup();
    await open(user);
    isNotANumberInput(box("Multiply by a number")());
  });
});

// ---------------------------------------------------------------------------------------------
// The generic `kind: "number"` field descriptor — today its only user is the progress chart's
// Goal, and a target of 0.85 or 1250.5 is an ordinary thing to want.
// ---------------------------------------------------------------------------------------------

describe("a numeric field descriptor — viz/fields", () => {
  const d = fieldDescriptor({
    key: "progress",
    prefix: "progress.",
    fields: [{ id: "goal", label: "Goal", kind: "number", placeholder: "Target value" }],
    buildSpec: (_cfg, shared, helpers) => helpers.oneRow(undefined, shared),
  });
  const Control = d.controls.find((c) => c.id === "goal")!.render;

  function Harness() {
    const [config, setConfig] = useState<Record<string, unknown>>({});
    return (
      <Control
        config={config}
        onChange={(p) => setConfig((c) => ({ ...c, ...p }))}
        columns={[]}
        metrics={[]}
        resultColumns={[]}
        rows={[]}
        temporalUnits={{}}
        unitRow={() => null}
      />
    );
  }

  it("keeps a half-written decimal", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await typesAHalfWrittenDecimal(user, box("Goal"));
  });

  it("is not a number input", () => {
    render(<Harness />);
    isNotANumberInput(box("Goal")());
  });
});

// ---------------------------------------------------------------------------------------------
// The shared control's own contract, away from any panel.
// ---------------------------------------------------------------------------------------------

describe("DecimalInput", () => {
  function Harness({ initial, unsetText }: { initial?: number; unsetText?: string }) {
    const [value, setValue] = useState<number | undefined>(initial);
    return <DecimalInput aria-label="n" value={value} onChange={setValue} unsetText={unsetText} />;
  }
  const n = box("n");

  it("shows what is stored, and the unset text when nothing is", () => {
    const { unmount } = render(<Harness initial={7.25} />);
    expect(n().value).toBe("7.25");
    unmount();
    render(<Harness unsetText="2.5" />);
    expect(n().value).toBe("2.5");
  });

  it("commits every parseable prefix and NEVER 0 while a decimal is half-written", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [value, setValue] = useState<number | undefined>(undefined);
      return (
        <DecimalInput
          aria-label="n"
          value={value}
          onChange={(v) => {
            seen.push(v);
            setValue(v);
          }}
        />
      );
    }
    render(<Spy />);
    await user.type(n(), "4");
    await user.type(n(), ".");
    await user.type(n(), "5");
    // "4." parses to 4 — the value does not move while the point sits there, and it never
    // collapses to 0 or to unset on the way.
    expect(seen).toEqual([4, 4, 4.5]);
  });

  it("emptying the box means unset, not 0", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [value, setValue] = useState<number | undefined>(12);
      return (
        <DecimalInput
          aria-label="n"
          value={value}
          onChange={(v) => {
            seen.push(v);
            setValue(v);
          }}
        />
      );
    }
    render(<Spy />);
    await user.clear(n());
    expect(seen).toEqual([undefined]);
    expect(n().value).toBe("");
  });

  // A box holding only spaces is an EMPTY box, and must reach `undefined` like any other. The
  // trap is that `Number(" ")` is 0 — the same coercion that started all of this — so an emptiness
  // test that forgets to trim turns "select all, press space" into the fold-nothing / zero-floor
  // setting rather than into unset.
  it("treats a whitespace-only box as empty rather than letting Number(' ') make it 0", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [value, setValue] = useState<number | undefined>(12);
      return (
        <DecimalInput
          aria-label="n"
          value={value}
          onChange={(v) => {
            seen.push(v);
            setValue(v);
          }}
        />
      );
    }
    render(<Spy />);
    await user.clear(n());
    seen.length = 0;
    await user.type(n(), " ");
    expect(seen).toEqual([undefined]);
    expect(seen).not.toContain(0);
  });

  it("does not commit text that is not a number, and puts the stored value back on blur", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [value, setValue] = useState<number | undefined>(10);
      return (
        <DecimalInput
          aria-label="n"
          value={value}
          onChange={(v) => {
            seen.push(v);
            setValue(v);
          }}
        />
      );
    }
    render(<Spy />);
    await user.type(n(), "x");
    expect(seen).toEqual([]);
    expect(n().value).toBe("10x"); // still visible, just not stored
    await user.tab();
    expect(n().value).toBe("10");
  });

  it("normalises a trailing point on blur so the box shows what is stored", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(n(), "4");
    await user.type(n(), ".");
    expect(n().value).toBe("4.");
    await user.tab();
    expect(n().value).toBe("4");
  });

  it("rejects Infinity, which parses but is no use as a bound or a goal", async () => {
    const user = userEvent.setup();
    const seen: (number | undefined)[] = [];
    function Spy() {
      const [value, setValue] = useState<number | undefined>(3);
      return (
        <DecimalInput
          aria-label="n"
          value={value}
          onChange={(v) => {
            seen.push(v);
            setValue(v);
          }}
        />
      );
    }
    render(<Spy />);
    await user.clear(n());
    seen.length = 0;
    await user.type(n(), "Infinity");
    expect(seen).toEqual([]);
  });

  it("still lets a negative number be typed, sign first", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(n(), "-");
    expect(n().value).toBe("-"); // not a number yet, but it has to stay in the box
    await user.type(n(), "3");
    await user.type(n(), ".");
    await user.type(n(), "5");
    expect(n().value).toBe("-3.5");
  });
});
