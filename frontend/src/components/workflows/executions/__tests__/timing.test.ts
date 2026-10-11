import { describe, expect, it } from "vitest";
import { executionDuration, executionStart, hasRun, startTitle } from "@/components/workflows/executions/timing";
import { fullTimestamp } from "@/lib/relativeTime";

const CREATED = "2026-10-11T10:00:00.000001Z";
const T = (iso: string) => new Date(iso).getTime();

/** A row with the timing fields only; each case sets what it is about. */
function row(over: {
  state: string;
  history: string[];
  started_at?: string | null;
  ended_at?: string | null;
  updated_at?: string;
}) {
  return {
    created_at: CREATED,
    updated_at: "2026-10-11T10:09:00.000001Z",
    started_at: null,
    ended_at: null,
    ...over,
  };
}

describe("hasRun", () => {
  it("is true once history holds RUNNING, wherever it sits", () => {
    expect(hasRun(row({ state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"] }))).toBe(true);
    expect(hasRun(row({ state: "FAILED", history: ["CREATED", "RUNNING", "RETRYING", "RUNNING", "FAILED"] }))).toBe(true);
  });

  it("is false for a row that never ran, and for a history that is not an array", () => {
    expect(hasRun(row({ state: "SKIPPED", history: ["CREATED", "SKIPPED"] }))).toBe(false);
    expect(hasRun(row({ state: "CANCELLED", history: ["CREATED", "CANCELLED"] }))).toBe(false);
    expect(hasRun({ ...row({ state: "CREATED", history: [] }), history: { RUNNING: true } })).toBe(false);
  });

  it("trusts a recorded start even when the history does not say RUNNING", () => {
    expect(hasRun(row({ state: "SUCCESS", history: [], started_at: "2026-10-11T10:00:02Z" }))).toBe(true);
  });
});

describe("executionDuration", () => {
  const NOW = T("2026-10-11T10:30:00Z");

  it("uses ended_at − started_at when both are recorded, exact, and ignores created_at and updated_at", () => {
    const d = executionDuration(
      row({ state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"], started_at: "2026-10-11T10:00:05Z", ended_at: "2026-10-11T10:01:10Z" }),
      NOW,
    );
    expect(d).toEqual({ ms: 65_000, estimated: false, running: false });
  });

  it("ticks from started_at to now while the row is not terminal", () => {
    const d = executionDuration(row({ state: "RUNNING", history: ["CREATED", "RUNNING"], started_at: "2026-10-11T10:29:00Z" }), NOW);
    expect(d).toEqual({ ms: 60_000, estimated: false, running: true });
  });

  it("keeps ticking for a paused row: it has a start and no end", () => {
    const d = executionDuration(row({ state: "PAUSED", history: ["CREATED", "RUNNING", "PAUSED"], started_at: "2026-10-11T10:29:30Z" }), NOW);
    expect(d).toEqual({ ms: 30_000, estimated: false, running: true });
  });

  it("in flight at deploy (no start, a real end): created_at to ended_at, estimated", () => {
    const d = executionDuration(
      row({ state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"], ended_at: "2026-10-11T10:02:00Z" }),
      NOW,
    );
    expect(d).toEqual({ ms: 120_000, estimated: true, running: false });
  });

  it("in flight at deploy and still running: created_at to now, estimated", () => {
    const d = executionDuration(row({ state: "RUNNING", history: ["CREATED", "RUNNING"] }), NOW);
    expect(d).toEqual({ ms: 30 * 60_000, estimated: true, running: true });
  });

  it("legacy (both null, terminal, ran): created_at to updated_at, estimated", () => {
    const d = executionDuration(row({ state: "FAILED", history: ["CREATED", "RUNNING", "FAILED"], updated_at: "2026-10-11T10:00:45Z" }), NOW);
    expect(d).toEqual({ ms: 45_000, estimated: true, running: false });
  });

  it("never ran: no duration, whatever the timestamps say", () => {
    for (const r of [
      row({ state: "CREATED", history: ["CREATED"] }),
      row({ state: "QUEUED", history: ["CREATED", "QUEUED"] }),
      row({ state: "SKIPPED", history: ["CREATED", "SKIPPED"], ended_at: "2026-10-11T10:00:01Z" }),
      row({ state: "CANCELLED", history: ["CREATED", "CANCELLED"], ended_at: "2026-10-11T10:05:00Z" }),
    ]) {
      expect(executionDuration(r, NOW)).toBeNull();
    }
  });

  it("a terminal row with a start but no end (should not happen) ends at updated_at, estimated", () => {
    const d = executionDuration(
      row({ state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"], started_at: "2026-10-11T10:00:10Z", updated_at: "2026-10-11T10:00:40Z" }),
      NOW,
    );
    expect(d).toEqual({ ms: 30_000, estimated: true, running: false });
  });
});

describe("executionStart", () => {
  it("is started_at when recorded", () => {
    expect(executionStart(row({ state: "RUNNING", history: ["CREATED", "RUNNING"], started_at: "2026-10-11T10:00:05Z" }))).toEqual({
      iso: "2026-10-11T10:00:05Z",
      kind: "started",
    });
  });

  it("is created_at, estimated, when the row ran without a recorded start", () => {
    expect(executionStart(row({ state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"] }))).toEqual({ iso: CREATED, kind: "estimated" });
  });

  it("is created_at, marked as not started, when the row never ran", () => {
    expect(executionStart(row({ state: "QUEUED", history: ["CREATED", "QUEUED"] }))).toEqual({ iso: CREATED, kind: "created" });
  });
});

describe("startTitle", () => {
  it("is the exact time for a recorded start, and says why otherwise", () => {
    expect(startTitle({ iso: "2026-10-11T10:00:05Z", kind: "started" })).toBe(fullTimestamp("2026-10-11T10:00:05Z"));
    expect(startTitle({ iso: CREATED, kind: "estimated" })).toBe(`Created ${fullTimestamp(CREATED)}. Estimated: the start was not recorded.`);
    expect(startTitle({ iso: CREATED, kind: "created" })).toBe(`Created ${fullTimestamp(CREATED)}; not started yet.`);
  });
});
