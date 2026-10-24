import { describe, it, expect } from "vitest";
import { humanizeCrontab } from "../humanizeCron";

describe("humanizeCrontab", () => {
  it("reads 'every minute' for * * * * *", () => {
    expect(humanizeCrontab("* * * * *")).toBe("Every minute");
  });

  it("reads 'every N minutes' for a minute step", () => {
    expect(humanizeCrontab("*/5 * * * *")).toBe("Every 5 minutes");
    expect(humanizeCrontab("*/15 * * * *")).toBe("Every 15 minutes");
  });

  it("treats a step of 1 the same as a bare wildcard", () => {
    expect(humanizeCrontab("*/1 * * * *")).toBe("Every minute");
  });

  it("reads a daily time for a single minute+hour", () => {
    expect(humanizeCrontab("0 9 * * *")).toBe("Every day at 09:00");
    expect(humanizeCrontab("30 14 * * *")).toBe("Every day at 14:30");
  });

  it("pads single-digit minutes and hours", () => {
    expect(humanizeCrontab("5 3 * * *")).toBe("Every day at 03:05");
  });

  it("reads a weekly time for a single weekday", () => {
    expect(humanizeCrontab("30 8 * * 1")).toBe("Every Monday at 08:30");
    expect(humanizeCrontab("0 0 * * 0")).toBe("Every Sunday at 00:00");
  });

  it("treats weekday 7 as Sunday, same as 0", () => {
    expect(humanizeCrontab("0 0 * * 7")).toBe("Every Sunday at 00:00");
  });

  it("reads a comma-separated weekday list", () => {
    expect(humanizeCrontab("0 8 * * 1,3,5")).toBe(
      "Every Monday, Wednesday, and Friday at 08:00",
    );
    expect(humanizeCrontab("0 8 * * 1,2")).toBe("Every Monday and Tuesday at 08:00");
  });

  it("dedupes and sorts an out-of-order weekday list", () => {
    // 7 and 0 both mean Sunday; listing both must not repeat "Sunday" twice.
    expect(humanizeCrontab("0 9 * * 5,0,7")).toBe("Every Sunday and Friday at 09:00");
  });

  // ── the fallback: a wrong humanisation is worse than the raw expression ──

  // KI-038: every shape the editor's picker can produce is described, plus
  // the weekday range the seeded "Late orders to Slack" uses.
  it("reads an hourly schedule (KI-038)", () => {
    expect(humanizeCrontab("0 * * * *")).toBe("Every hour at minute 0");
    expect(humanizeCrontab("15 * * * *")).toBe("Every hour at minute 15");
  });

  it("reads a weekday range (KI-038)", () => {
    expect(humanizeCrontab("30 8 * * 1-5")).toBe("Every Monday to Friday at 08:30");
    expect(humanizeCrontab("0 9 * * 0-6")).toBe("Every Sunday to Saturday at 09:00");
  });

  it("reads a weekday list that mixes a range and single days (KI-038)", () => {
    expect(humanizeCrontab("0 9 * * 1-3,5")).toBe(
      "Every Monday, Tuesday, Wednesday, and Friday at 09:00",
    );
  });

  it("reads a monthly schedule (KI-038)", () => {
    expect(humanizeCrontab("0 9 1 * *")).toBe("Every month on day 1 at 09:00");
    expect(humanizeCrontab("30 6 28 * *")).toBe("Every month on day 28 at 06:30");
  });

  it("reads a yearly schedule (KI-038)", () => {
    expect(humanizeCrontab("0 8 1 1 *")).toBe("Every year on 1 January at 08:00");
  });

  it("keeps the raw expression for a day-of-month crossed with a weekday, which cron ORs", () => {
    const raw = "0 9 1 * 1";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("keeps the raw expression for a range it will not read", () => {
    // Backwards, out of bounds, or 7 (Sunday) as a range end — each stays raw.
    for (const raw of ["0 9 * * 5-1", "0 9 * * 1-8", "0 9 * * 5-7", "0 9 1-5 * *"]) {
      expect(humanizeCrontab(raw)).toBe(raw);
    }
  });

  it("falls back to the raw expression for a month restriction without a day", () => {
    const raw = "0 9 * 6 *";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  // NOTE: an hour step used to fall back, deliberately. It is now described —
  // see "reads an hourly step with a minute offset" below, which replaces this
  // case, and the sibling test that pins what still stays raw. Kept as a
  // comment rather than deleted so the change of mind is visible: the shape
  // reached the page header, where a raw expression is the only statement of
  // when the schedule runs.

  it("falls back to the raw expression for a minute list", () => {
    const raw = "0,30 9 * * *";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("reads an hourly step with a minute offset", () => {
    // `15 */6 * * *` fires 00:15, 06:15, 12:15, 18:15 — the shape the seeded
    // schedules use, and the one the page header showed raw.
    expect(humanizeCrontab("15 */6 * * *")).toBe("Every 6 hours, at minute 15");
    expect(humanizeCrontab("0 */2 * * *")).toBe("Every 2 hours, at minute 0");
    expect(humanizeCrontab("30 */1 * * *")).toBe("Every hour at minute 30");
  });

  it("will not describe an hourly step it has not been proven against", () => {
    // A step crossed with a weekday list, a non-numeric minute, or a step
    // outside the hour range: each stays raw rather than guessing, because
    // this text tells someone when their mail arrives.
    for (const raw of ["15 */6 * * 1-5", "*/5 */6 * * *", "15 */0 * * *", "15 */24 * * *"]) {
      expect(humanizeCrontab(raw)).toBe(raw);
    }
  });

  it("falls back to the raw expression for an out-of-range field count", () => {
    expect(humanizeCrontab("* * * *")).toBe("* * * *");
    expect(humanizeCrontab("* * * * * *")).toBe("* * * * * *");
  });

  it("falls back to the raw expression for garbage input rather than throwing", () => {
    expect(humanizeCrontab("not a cron")).toBe("not a cron");
  });

  it("trims surrounding whitespace before deciding, but returns it collapsed", () => {
    expect(humanizeCrontab("  0   9   *   *   *  ")).toBe("Every day at 09:00");
  });
});
