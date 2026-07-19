import { describe, it, expect } from "vitest";
import { defaultSqlInto } from "@/api/notebooks";
import type { Cell } from "@/api/notebooks";

function cell(sql_into?: string): Cell {
  return {
    id: Math.random().toString(),
    cell_type: "sql",
    source: "",
    cell_order: 0,
    execution_count: null,
    last_output: null,
    last_execution_time_ms: null,
    sql_into,
  };
}

describe("defaultSqlInto", () => {
  it("returns df when none used", () => {
    expect(defaultSqlInto([])).toBe("df");
  });
  it("skips taken names", () => {
    expect(defaultSqlInto([cell("df"), cell("df_2")])).toBe("df_3");
  });
});
