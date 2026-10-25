import { describe, it, expect } from "vitest";
import { fromCron, toCron, DEFAULT_SCHEDULE } from "@/lib/cronSchedule";

describe("toCron", () => {
  it("names a period outside the union instead of returning undefined", () => {
    const bad = { ...DEFAULT_SCHEDULE, period: "" } as unknown as typeof DEFAULT_SCHEDULE;
    expect(() => toCron(bad)).toThrow('Unknown schedule period: ""');
  });

  it("writes each period's shape", () => {
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "minutes", every: 15 })).toBe("*/15 * * * *");
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "hourly", every: 1, minute: 5 })).toBe(
      "5 * * * *",
    );
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "hourly", every: 6, minute: 15 })).toBe(
      "15 */6 * * *",
    );
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "daily", hour: 9, minute: 0 })).toBe("0 9 * * *");
    expect(
      toCron({ ...DEFAULT_SCHEDULE, period: "weekly", hour: 8, minute: 30, weekday: 1 }),
    ).toBe("30 8 * * 1");
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "monthly", hour: 7, minute: 0, day: 1 })).toBe(
      "0 7 1 * *",
    );
  });

  it("clamps rather than emitting an out-of-range field", () => {
    // A cron field the scheduler cannot parse would leave the schedule with a
    // NULL next_run_at — created, and never firing.
    expect(toCron({ ...DEFAULT_SCHEDULE, period: "daily", hour: 99, minute: -3 })).toBe(
      "0 23 * * *",
    );
  });
});

describe("fromCron", () => {
  it("round-trips every shape toCron can write", () => {
    const cases = [
      { ...DEFAULT_SCHEDULE, period: "minutes" as const, every: 15 },
      { ...DEFAULT_SCHEDULE, period: "hourly" as const, every: 1, minute: 5 },
      { ...DEFAULT_SCHEDULE, period: "hourly" as const, every: 6, minute: 15 },
      { ...DEFAULT_SCHEDULE, period: "daily" as const, hour: 9, minute: 0 },
      { ...DEFAULT_SCHEDULE, period: "weekly" as const, hour: 8, minute: 30, weekday: 3 },
      { ...DEFAULT_SCHEDULE, period: "monthly" as const, hour: 7, minute: 0, day: 12 },
    ];
    for (const c of cases) {
      const back = fromCron(toCron(c));
      expect(back, `round trip failed for ${toCron(c)}`).not.toBeNull();
      expect(toCron(back!)).toBe(toCron(c));
    }
  });

  it("returns null for expressions the dropdowns cannot express", () => {
    // Each of these would have to be silently rewritten to fit the picker,
    // which would change when someone's mail arrives.
    for (const expr of [
      "0 9 * * 1-5", // weekday range
      "0 9,17 * * *", // list of hours
      "0 9 1 1 *", // month restriction
      "0 9 1 * 1", // day-of-month AND weekday: cron ORs them
      "0 9 * *", // too few fields
      "*/0 * * * *", // zero step
    ]) {
      expect(fromCron(expr), `${expr} should not be representable`).toBeNull();
    }
  });

  it("reads a plain hourly expression as hourly, not as daily at midnight", () => {
    const s = fromCron("30 * * * *");
    expect(s?.period).toBe("hourly");
    expect(s?.every).toBe(1);
    expect(s?.minute).toBe(30);
  });
});
