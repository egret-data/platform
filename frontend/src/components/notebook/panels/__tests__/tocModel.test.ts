import { describe, it, expect } from "vitest";
import { deriveToc } from "../tocModel";
import type { Cell } from "@/api/notebooks";

function cell(partial: Partial<Cell> & Pick<Cell, "id" | "cell_type" | "source" | "cell_order">): Cell {
  return { execution_count: null, last_output: null, last_execution_time_ms: null, ...partial };
}

describe("deriveToc", () => {
  it("extracts markdown headings with levels and labels code cells", () => {
    const cells: Cell[] = [
      cell({ id: "a", cell_type: "markdown", source: "# Title\nbody", cell_order: 0 }),
      cell({ id: "b", cell_type: "markdown", source: "## Section", cell_order: 1 }),
      cell({ id: "c", cell_type: "code", source: "df = trino('SELECT 1')\nmore", cell_order: 2, execution_count: 4 }),
    ];
    const toc = deriveToc(cells);
    expect(toc).toEqual([
      { id: "a", label: "Title", level: 1 },
      { id: "b", label: "Section", level: 2 },
      { id: "c", label: "In [4] · df = trino('SELECT 1')", level: 3 },
    ]);
  });
});
