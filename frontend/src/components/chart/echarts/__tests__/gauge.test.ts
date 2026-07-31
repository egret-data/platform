import { describe, it, expect } from "vitest";
import { buildGaugeOption, defaultGaugeSegments } from "../options/gauge";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "rev", type: "double" }];

// Today's fixed zones (task 1's shim, made explicit here so test 1 pins the exact numbers).
const ZONE_LOW = SERIES_PALETTE[3];  // red
const ZONE_MID = SERIES_PALETTE[4];  // yellow
const ZONE_HIGH = SERIES_PALETTE[1]; // green

describe("buildGaugeOption", () => {
  describe("no segments configured (unconfigured gauge)", () => {
    // Metabase's own default: the arc runs 0 -> 2v with boundaries at v/2 and v, so the needle
    // lands exactly on the middle boundary. Equal thirds put it at an arbitrary point and made the
    // boundaries fractional for most values.
    it("splits [0, value*2] at value/2 and value, red -> yellow -> green", () => {
      const opt = buildGaugeOption(cols, [[60]], { metric: "rev" }) as any;
      expect(opt.series[0].type).toBe("gauge");
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([
        [1 / 4, ZONE_LOW],
        [1 / 2, ZONE_MID],
        [1, ZONE_HIGH],
      ]);
      expect(opt.series[0].data[0].value).toBe(60);
    });

    it("falls back to a range of 100 when the value is zero or negative", () => {
      expect((buildGaugeOption(cols, [[0]], { metric: "rev" }) as any).series[0].max).toBe(100);
      expect((buildGaugeOption(cols, [[-5]], { metric: "rev" }) as any).series[0].max).toBe(100);
    });

    it("ignores a min/max left over from before segments existed", () => {
      // Stored settings are not whitelisted server-side, so these keys can still arrive at
      // runtime on an old chart even though GaugeBuildConfig no longer declares them.
      const opt = buildGaugeOption(cols, [[60]], { metric: "rev", min: 999, max: 5000 } as any) as any;
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
    });

    it("falls back to a zero value when there is no metric/row", () => {
      expect((buildGaugeOption(cols, [], {}) as any).series[0].data[0].value).toBe(0);
    });

    it("formats the centre detail via column settings", () => {
      const opt = buildGaugeOption(cols, [[1000]], { metric: "rev", metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$", decimals: 0 } } }) as any;
      expect(opt.series[0].detail.formatter(1000)).toBe("$1,000");
    });
  });

  describe("segments configured", () => {
    it("derives the scale from segments: smallest min, largest max", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 10, max: 50, color: "#000000" },
          { min: 50, max: 90, color: "#111111" },
        ],
      }) as any;
      expect(opt.series[0].min).toBe(10);
      expect(opt.series[0].max).toBe(90);
    });

    it("maps segments to axisLine colour stops as fractions of the derived range", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "green" },
          { min: 50, max: 100, color: "red" },
        ],
      }) as any;
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[0.5, "green"], [1, "red"]]);
    });

    it("supports the less-is-better direction: red at the top of the arc, green at the bottom", () => {
      // This is the defect the unit exists to fix: colour direction follows the segments as
      // configured, never a hardcoded low=green/high=red assumption.
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "green" },  // low latency: good
          { min: 50, max: 100, color: "red" },  // high latency: bad
        ],
      }) as any;
      const stops = opt.series[0].axisLine.lineStyle.color;
      expect(stops[0]).toEqual([0.5, "green"]);
      expect(stops[stops.length - 1]).toEqual([1, "red"]);
    });

    it("sorts out-of-order segments before building stops", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 50, max: 100, color: "red" },
          { min: 0, max: 50, color: "green" },
        ],
      }) as any;
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[0.5, "green"], [1, "red"]]);
    });

    it("a single segment fills the whole arc", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [{ min: 0, max: 100, color: "blue" }],
      }) as any;
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(100);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[1, "blue"]]);
    });

    it("does not divide by zero when the derived range collapses to a single point", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [{ min: 5, max: 5, color: "teal" }],
      }) as any;
      expect(opt.series[0].min).toBe(5);
      expect(opt.series[0].max).toBe(5);
      const stops = opt.series[0].axisLine.lineStyle.color as [number, string][];
      expect(stops.every(([f]) => Number.isFinite(f))).toBe(true);
    });

    it("makes a segment's label reachable via the aria description", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "green", label: "Healthy" },
          { min: 50, max: 100, color: "red", label: "Critical" },
        ],
      }) as any;
      expect(opt.aria.label.description).toContain("Healthy");
      expect(opt.aria.label.description).toContain("Critical");
    });
  });

  describe("malformed segments (unwhitelisted stored JSONB)", () => {
    it("drops a segment with non-finite or missing bounds, keeping the rest", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "green" },
          { min: NaN, max: 100, color: "red" } as any,
          { max: 100, color: "orange" } as any,          // missing min
          { min: "nope" as any, max: 100, color: "purple" },
          { min: null as any, max: 100, color: "gray" },
        ],
      }) as any;
      // Only the valid segment survives, so the range is that segment's own bounds.
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(50);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[1, "green"]]);
    });

    it("drops a segment whose min exceeds its max", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 100, max: 0, color: "red" },
          { min: 0, max: 50, color: "green" },
        ],
      }) as any;
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[1, "green"]]);
    });

    it("keeps a segment whose colour isn't a colour, resolving it to a palette fallback rather than dropping the zone", () => {
      // A bad colour is a rendering problem, not a reason to throw away the bounds the user
      // configured — resolveColor guarantees a drawable value, so the segment survives.
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "not-a-real-color" },
          { min: 50, max: 100, color: "green" },
        ],
      }) as any;
      const stops = opt.series[0].axisLine.lineStyle.color;
      expect(stops).toHaveLength(2);
      expect(stops[0][0]).toBe(0.5);
      expect(stops[0][1]).not.toBe("not-a-real-color");
      expect(stops[1]).toEqual([1, "green"]);
    });

    it("resolves a var(--token) segment colour rather than passing the literal through unresolved", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [{ min: 0, max: 100, color: "var(--some-brand-token)" }],
      }) as any;
      const [, color] = opt.series[0].axisLine.lineStyle.color[0];
      expect(color).not.toContain("var(");
    });

    it("falls back to the default zones when every segment is invalid", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: NaN, max: 100, color: "red" } as any,
          { min: 0, max: "nope" as any, color: "blue" },
        ],
      }) as any;
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([
        [1 / 4, ZONE_LOW],
        [1 / 2, ZONE_MID],
        [1, ZONE_HIGH],
      ]);
    });

    it("falls back to the default zones for an empty segments array", () => {
      const opt = buildGaugeOption(cols, [[60]], { metric: "rev", segments: [] }) as any;
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
    });

    it.each([
      ["an empty string", ""],
      ["a numeric string", "12"],
      ["undefined", undefined],
      ["null", null],
      ["NaN", NaN],
    ])("drops a segment whose min is %s rather than coercing it to a number", (_label, badMin) => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: badMin as any, max: 50, color: "purple" },
          { min: 50, max: 100, color: "green" },
        ],
      }) as any;
      // Only the valid segment survives, so the range is that segment's own bounds.
      expect(opt.series[0].min).toBe(50);
      expect(opt.series[0].max).toBe(100);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[1, "green"]]);
    });

    it("resolves an overlap by upper bound: the smaller max claims the disputed range, the larger max gets what's left", () => {
      // [0,60] green and [40,100] red overlap on 40-60. ECharts' stop model has no notion of
      // a segment's own floor — sorted by max, each entry paints up to its own max and the
      // next entry starts there — so green (max 60, sorted first) silently wins the whole
      // 0-60 stretch including the part red's min=40 meant to own; red only gets 60-100. A
      // value of 45 (which the red segment was configured to call dangerous) therefore
      // renders inside the green zone. Task 4's editor keeps authored ranges contiguous so
      // this can't be created from the UI; this pins what happens if it arrives anyway.
      const opt = buildGaugeOption(cols, [[45]], {
        metric: "rev",
        segments: [
          { min: 0, max: 60, color: "green" },
          { min: 40, max: 100, color: "red" },
        ],
      }) as any;
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[0.6, "green"], [1, "red"]]);
    });
  });
});

describe("defaultGaugeSegments", () => {
  // The whole point of the v/2 · v · 2v split: the needle sits ON a boundary, so a reader sees
  // where they are without reading a number.
  it("puts the value itself on the middle boundary", () => {
    expect(defaultGaugeSegments(18760)).toEqual([
      { min: 0, max: 9380, color: ZONE_LOW },
      { min: 9380, max: 18760, color: ZONE_MID },
      { min: 18760, max: 37520, color: ZONE_HIGH },
    ]);
  });

  // Equal thirds of 0 -> 2v filled the editor's number inputs with `77512.66666666667` for this
  // value — eleven decimals the user had to clear by hand before the panel read as anything but
  // broken. Rounding happens here, not in the input, because the number the editor SHOWS is the
  // number stored the moment any other field on the row is touched.
  it("keeps derived bounds to something a person would type", () => {
    for (const s of defaultGaugeSegments(116269)) {
      for (const b of [s.min, s.max]) {
        expect(String(b)).toMatch(/^-?\d+(\.\d{1,2})?$/);
      }
    }
    expect(defaultGaugeSegments(116269)[1].max).toBe(116269);
  });

  it("keeps three decimals for a sub-unit value rather than collapsing it to zero", () => {
    const [low] = defaultGaugeSegments(0.42);
    expect(low.max).toBe(0.21);
  });

  it("falls back to a 0-100 arc when there is no value to scale to", () => {
    expect(defaultGaugeSegments(0).at(-1)!.max).toBe(100);
  });
});
