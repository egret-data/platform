import { describe, expect, it } from "vitest";
import { labelsFromRows, rowErrors, rowsChanged, rowsFromLabels } from "@/components/workflows/executions/labelsModel";

describe("labelsModel", () => {
  const labels = [
    { key: "team", value: "data" },
    { key: "system.restarted", value: "true" },
  ];
  it("marks system rows and round-trips", () => {
    const rows = rowsFromLabels(labels);
    expect(rows.map((r) => r.system)).toEqual([false, true]);
    expect(labelsFromRows(rows)).toEqual(labels);
    expect(rowsChanged(labels, rows)).toBe(false);
  });
  it("finds empty keys, empty values and repeated keys, by row", () => {
    const rows = [
      { key: "", value: "x", system: false },
      { key: "a", value: "", system: false },
      { key: "a", value: "1", system: false },
    ];
    const errors = rowErrors(rows);
    expect(errors.get(0)).toEqual({ key: "Enter a key" });
    expect(errors.get(1)).toEqual({ value: "Enter a value" });
    expect(errors.get(2)).toEqual({ key: "This key is already used" });
  });
  it("sees a removal as a change", () => {
    expect(rowsChanged(labels, rowsFromLabels(labels.slice(1)))).toBe(true);
  });
});
