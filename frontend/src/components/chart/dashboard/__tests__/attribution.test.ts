import { describe, it, expect } from "vitest";
import { attributionLine, revisionSummary } from "../attribution";

describe("attributionLine", () => {
  // 12:00 UTC on purpose: the local calendar day for that instant is Aug 1 or Aug 2 in every
  // timezone on earth (UTC-12 … UTC+14), so the month and year can be pinned without pinning the
  // test to the machine's TZ. Asserting a hard-coded "August 1, 2026" would be green in CI and
  // red for anyone west of UTC-12... and asserting toLocaleDateString() against itself would
  // assert nothing at all.
  const iso = "2026-08-01T12:00:00Z";

  it("renders the date and the name, Metabase's wording", () => {
    expect(attributionLine(iso, "Ref User")).toMatch(/^August [12], 2026 by Ref User$/);
  });

  it("drops the 'by' clause when there is no name rather than printing 'by null'", () => {
    const out = attributionLine(iso, null);
    expect(out).toMatch(/^August [12], 2026$/);
    expect(out).not.toContain("by");
  });

  it("renders nothing at all when there is no date", () => {
    // A dashboard predating migration 0106 has created_at NULL. That is a gap in what was
    // recorded, not a fact about the dashboard, so the row is omitted — not shown as "—" beside
    // a name that would then look like it belonged to a date.
    expect(attributionLine(null, "Ref User")).toBeNull();
  });

  it("renders nothing for an unparseable date", () => {
    expect(attributionLine("not-a-date", "Ref User")).toBeNull();
  });
});

describe("revisionSummary", () => {
  it("uses Metabase's three phrasings, keyed on the RAW kind", () => {
    // Keyed on the raw enum from the API, never on a display string. Three of the six
    // stubbed-assertion defects in the chart programme were tests that read a display string
    // where the code keyed on a raw one.
    expect(revisionSummary("baseline", "Ref User")).toBe("Ref User created this");
    expect(revisionSummary("edit", "Ref User")).toBe("Ref User edited this");
    expect(revisionSummary("revert", "Ref User")).toBe("Ref User reverted to an earlier version");
  });

  it("says 'Someone' rather than dropping the sentence when the author is gone", () => {
    // created_by is NULL after a hard delete (0093's ON DELETE SET NULL), and a timeline row with
    // no subject reads as a bug.
    expect(revisionSummary("edit", null)).toBe("Someone edited this");
  });

  it("degrades to a neutral phrasing for a kind it does not know", () => {
    expect(revisionSummary("something-new", "Ref User")).toBe("Ref User changed this");
  });
});
