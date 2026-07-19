import { describe, it, expect } from "vitest";
import { reorderIds } from "@/lib/reorderIds";

describe("reorderIds", () => {
  it("moves an id down past a later one", () => {
    expect(reorderIds(["a", "b", "c"], "a", "c")).toEqual(["b", "c", "a"]);
  });

  it("moves an id up past an earlier one", () => {
    expect(reorderIds(["a", "b", "c"], "c", "a")).toEqual(["c", "a", "b"]);
  });

  it("moves an id by one position down", () => {
    expect(reorderIds(["a", "b", "c"], "a", "b")).toEqual(["b", "a", "c"]);
  });

  it("is a no-op (same reference) when active and over are the same id", () => {
    const ids = ["a", "b", "c"];
    expect(reorderIds(ids, "b", "b")).toBe(ids);
  });

  it("is a no-op (same reference) when the active id is not found", () => {
    const ids = ["a", "b", "c"];
    expect(reorderIds(ids, "missing", "b")).toBe(ids);
  });

  it("is a no-op (same reference) when the over id is not found", () => {
    const ids = ["a", "b", "c"];
    expect(reorderIds(ids, "a", "missing")).toBe(ids);
  });
});
