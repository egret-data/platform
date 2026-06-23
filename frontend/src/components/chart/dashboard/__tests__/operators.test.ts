import { describe, it, expect } from "vitest";
import { OPERATORS } from "../ParameterEditor";

describe("OPERATORS", () => {
  it("has boolean and location types", () => {
    expect(OPERATORS.boolean).toEqual(["is"]);
    expect(OPERATORS.location).toEqual(["is", "is-not"]);
  });
  it("adds month-year and quarter-year to date, keeps does-not-contain absent", () => {
    expect(OPERATORS.date).toContain("month-year");
    expect(OPERATORS.date).toContain("quarter-year");
    expect(OPERATORS.text).not.toContain("does-not-contain");
  });
  it("date includes the all-options operator", () => {
    expect(OPERATORS.date).toContain("all-options");
  });
});
