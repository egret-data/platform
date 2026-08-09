import { describe, it, expect } from "vitest";
import { countChangedFilters, describeFilterChanges } from "../filterDrafts";

describe("countChangedFilters", () => {
  it("counts additions, removals and edits, and ignores untouched values", () => {
    expect(countChangedFilters({ a: "x" }, { a: "x" })).toBe(0);
    expect(countChangedFilters({}, { a: "x" })).toBe(1);                       // added
    expect(countChangedFilters({ a: "x" }, { a: "" })).toBe(1);                // removed
    expect(countChangedFilters({ a: "x" }, { a: "y" })).toBe(1);               // edited
    expect(countChangedFilters({ a: "x", b: "1" }, { a: "y", b: "2" })).toBe(2);
  });

  it("compares arrays by content, not by reference", () => {
    expect(countChangedFilters({ a: ["x", "y"] }, { a: ["x", "y"] })).toBe(0);
    expect(countChangedFilters({ a: ["x"] }, { a: ["x", "y"] })).toBe(1);
  });

  it("treats every shape of empty as the same state", () => {
    // "" / undefined / [] are one state here, because `setValue` deletes all three from the URL —
    // reporting a change the URL cannot represent would leave an Apply button that does nothing.
    expect(countChangedFilters({ a: "" }, {})).toBe(0);
    expect(countChangedFilters({ a: [] }, { a: undefined })).toBe(0);
  });
});

describe("describeFilterChanges", () => {
  it("is singular at one and plural otherwise", () => {
    expect(describeFilterChanges(1)).toBe("1 filter changed");
    expect(describeFilterChanges(3)).toBe("3 filters changed");
  });
});
