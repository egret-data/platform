import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { CellEditor } from "../CellEditor";
import type { Cell } from "@/api/notebooks";

vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v?: string) => void }) => (
    <textarea aria-label="code" value={value} onChange={(e) => onChange?.(e.target.value)} />
  ),
}));

function cell(partial: Partial<Cell> & Pick<Cell, "id" | "cell_type" | "source" | "cell_order">): Cell {
  return { execution_count: null, last_output: null, last_execution_time_ms: null, ...partial };
}

const noop = () => {};
const baseProps = {
  onChangeSource: noop,
  onChangeType: noop,
  onDelete: noop,
  onMoveUp: noop,
  onMoveDown: noop,
  engines: [],
  onChangeSqlTarget: noop,
  onChangeSqlInto: noop,
};

describe("CellEditor", () => {
  it("shows the In [N] execution prompt for code cells", () => {
    render(
      <CellEditor
        {...baseProps}
        cell={cell({ id: "a", cell_type: "code", source: "x = 1", cell_order: 0, execution_count: 7 })}
      />,
    );
    expect(screen.getByText("In [7]:")).toBeInTheDocument();
  });

  it("renders markdown as preview when the source is set and cell is unfocused", () => {
    render(
      <CellEditor
        {...baseProps}
        cell={cell({ id: "b", cell_type: "markdown", source: "# Hello", cell_order: 0 })}
      />,
    );
    expect(screen.getByRole("heading", { name: "Hello" })).toBeInTheDocument();
  });

  it("keeps the markdown textarea mounted while typing into an empty cell", () => {
    const changes: string[] = [];
    render(
      <CellEditor
        {...baseProps}
        onChangeSource={(s) => changes.push(s)}
        cell={cell({ id: "e", cell_type: "markdown", source: "", cell_order: 0 })}
      />,
    );
    const ta = screen.getByLabelText("Markdown source");
    fireEvent.focus(ta);
    fireEvent.change(ta, { target: { value: "h" } });
    expect(screen.getByLabelText("Markdown source")).toBeInTheDocument();
  });

  it("shows a target dropdown and variable input for SQL cells", () => {
    render(
      <CellEditor
        {...baseProps}
        engines={[{ id: "e1", name: "Trino", engine: "trino", enabled: true }]}
        cell={cell({ id: "s", cell_type: "sql", source: "SELECT 1", cell_order: 0, sql_target: "e1", sql_into: "df" })}
      />,
    );
    expect(screen.getByLabelText(/result variable/i)).toHaveValue("df");
  });
});
