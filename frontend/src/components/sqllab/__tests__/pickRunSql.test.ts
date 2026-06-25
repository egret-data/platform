import { describe, it, expect } from "vitest";
import { pickRunSql } from "../pickRunSql";

describe("pickRunSql", () => {
  const full = "SELECT 1;\nSELECT 2;";
  it("returns the selection when it has non-whitespace content", () => {
    expect(pickRunSql(full, "SELECT 2;")).toBe("SELECT 2;");
  });
  it("returns the full text when the selection is empty/whitespace/nullish", () => {
    expect(pickRunSql(full, "")).toBe(full);
    expect(pickRunSql(full, "   \n ")).toBe(full);
    expect(pickRunSql(full, null)).toBe(full);
    expect(pickRunSql(full, undefined)).toBe(full);
  });
  it("returns the selection verbatim (preserves surrounding whitespace)", () => {
    expect(pickRunSql(full, "  SELECT 2  ")).toBe("  SELECT 2  ");
  });
});
