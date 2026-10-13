import { describe, expect, it } from "vitest";
import { attemptsText, durationText, iterationsText, shortType } from "@/components/workflows/topology/topologyModel";
import { state } from "./fixtures";

describe("topologyModel", () => {
  it("shortens a type to its last segment and keeps an alias", () => {
    expect(shortType("io.kestra.plugin.core.log.Log")).toBe("Log");
    expect(shortType("Subflow")).toBe("Subflow");
  });

  it("counts iterations: done is finished without failing; failed and killed are apart; running is neither", () => {
    expect(iterationsText({ total: 3, by_state: { SUCCESS: 2, FAILED: 1 } })).toBe("2 of 3 iterations done, 1 failed");
    expect(iterationsText({ total: 4, by_state: { SUCCESS: 1, WARNING: 1, RUNNING: 1, KILLED: 1 } })).toBe("2 of 4 iterations done, 1 killed");
    expect(iterationsText({ total: 1, by_state: { RUNNING: 1 } })).toBe("0 of 1 iteration done");
  });

  it("says a fold's attempts are a maximum, a single run's a count, and nothing for one attempt", () => {
    expect(attemptsText(state("FAILED", { attempts: 3, run_id: null, iterations: { total: 2, by_state: { FAILED: 2 }, runs: [] } }))).toBe("up to 3 attempts");
    expect(attemptsText(state("FAILED", { attempts: 2 }))).toBe("2 attempts");
    expect(attemptsText(state("SUCCESS", { attempts: 1 }))).toBeNull();
  });

  it("measures a running node to now, a finished one to its end, and nothing before it started", () => {
    const started = Date.parse("2026-10-13T10:00:00Z");
    expect(durationText(state("RUNNING", { ended_at: null }), started + 65_000)).toBe("1 min 5 s so far");
    expect(durationText(state("SUCCESS"), started + 999_000)).toBe("3 s");
    expect(durationText(state("CREATED", { started_at: null, ended_at: null }), started)).toBeNull();
  });
});
