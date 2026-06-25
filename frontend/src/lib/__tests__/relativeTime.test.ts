import { describe, it, expect } from "vitest";
import { timeAgo, fullTimestamp } from "../relativeTime";

describe("relativeTime", () => {
  it("timeAgo renders a relative phrase ending in 'ago' for a past time", () => {
    const fiveMinAgo = new Date(Date.now() - 5 * 60_000).toISOString();
    expect(timeAgo(fiveMinAgo)).toMatch(/ago$/);
    expect(timeAgo(fiveMinAgo)).toContain("minute");
  });
  it("fullTimestamp formats as YYYY-MM-DD HH:mm:ss", () => {
    expect(fullTimestamp("2026-06-25T09:40:00Z")).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  });
});
