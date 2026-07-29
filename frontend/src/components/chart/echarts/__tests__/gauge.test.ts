import { describe, it, expect } from "vitest";
import { buildGaugeOption } from "../options/gauge";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "rev", type: "double" }];

// Today's fixed zones (task 1's shim, made explicit here so test 1 pins the exact numbers).
const ZONE_LOW = SERIES_PALETTE[3];  // red
const ZONE_MID = SERIES_PALETTE[4];  // yellow
const ZONE_HIGH = SERIES_PALETTE[1]; // green

describe("buildGaugeOption", () => {
  describe("no segments configured (unconfigured gauge)", () => {
    it("renders byte-identically to today: thirds of [0, value*2], red -> yellow -> green", () => {
      const opt = buildGaugeOption(cols, [[60]], { metric: "rev" }) as any;
      expect(opt.series[0].type).toBe("gauge");
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([
        [1 / 3, ZONE_LOW],
        [2 / 3, ZONE_MID],
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

    it("drops a segment whose colour isn't a colour", () => {
      const opt = buildGaugeOption(cols, [[60]], {
        metric: "rev",
        segments: [
          { min: 0, max: 50, color: "not-a-real-color" },
          { min: 50, max: 100, color: "green" },
        ],
      }) as any;
      expect(opt.series[0].axisLine.lineStyle.color).toEqual([[1, "green"]]);
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
        [1 / 3, ZONE_LOW],
        [2 / 3, ZONE_MID],
        [1, ZONE_HIGH],
      ]);
    });

    it("falls back to the default zones for an empty segments array", () => {
      const opt = buildGaugeOption(cols, [[60]], { metric: "rev", segments: [] }) as any;
      expect(opt.series[0].min).toBe(0);
      expect(opt.series[0].max).toBe(120);
    });
  });
});
