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

  it("falls back to the raw expression for a weekday range, which it does not attempt to understand", () => {
    // "1-5" (Mon-Fri) is a shape this helper deliberately does not claim to
    // read. This is the load-bearing case: humanizeCrontab must return the
    // RAW string unchanged, not a guess.
    const raw = "0 9 * * 1-5";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("falls back to the raw expression for a day-of-month restriction", () => {
    const raw = "0 9 1 * *";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("falls back to the raw expression for a month restriction", () => {
    const raw = "0 9 * 6 *";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("falls back to the raw expression for an hour step", () => {
    const raw = "0 */2 * * *";
    expect(humanizeCrontab(raw)).toBe(raw);
  });

  it("falls back to the raw expression for a minute list", () => {
    const raw = "0,30 9 * * *";
    expect(humanizeCrontab(raw)).toBe(raw);
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
