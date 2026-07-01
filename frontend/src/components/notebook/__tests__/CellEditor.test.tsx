import type { ComponentProps } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

// jsdom cannot load Monaco.
vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange: (v: string) => void }) => (
    <textarea data-testid="monaco" value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

import { CellEditor } from "@/components/notebook/CellEditor";
import type { Cell } from "@/api/notebooks";

const codeCell: Cell = {
  id: "cell-1",
  cell_type: "code",
  source: "print(1)",
  cell_order: 0,
  execution_count: null,
  last_output: null,
  last_execution_time_ms: null,
};

function renderCell(overrides: Partial<ComponentProps<typeof CellEditor>> = {}) {
  render(
    <form>
      <CellEditor
        cell={codeCell}
        onChangeSource={vi.fn()}
        onChangeType={vi.fn()}
        onDelete={vi.fn()}
        onMoveUp={vi.fn()}
        onMoveDown={vi.fn()}
        {...overrides}
      />
    </form>,
  );
}

describe("CellEditor cell-type select (dataframe-first, no SQL)", () => {
  it("offers only Code and Markdown as cell types", () => {
    renderCell();
    // The Radix Select renders a hidden native <select> inside the <form>.
    const native = Array.from(
      document.querySelectorAll<HTMLSelectElement>("select"),
    ).find((s) => Array.from(s.options).some((o) => o.value === "code"));
    expect(native).toBeTruthy();
    const values = Array.from(native!.options).map((o) => o.value);
    expect(values).toEqual(["code", "markdown"]);
  });

  it("does not render a SQL target control for a code cell", () => {
    renderCell();
    expect(screen.queryByLabelText("SQL target")).not.toBeInTheDocument();
  });

  it("does not render a SQL target control for a markdown cell", () => {
    renderCell({ cell: { ...codeCell, cell_type: "markdown" } });
    expect(screen.queryByLabelText("SQL target")).not.toBeInTheDocument();
  });
});
