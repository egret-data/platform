import type { ComponentProps } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// jsdom cannot load Monaco.
vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange: (v: string) => void }) => (
    <textarea data-testid="monaco" value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

import { CellEditor } from "@/components/notebook/CellEditor";
import type { Cell, EngineOption } from "@/api/notebooks";

const sqlCell: Cell = {
  id: "cell-1",
  cell_type: "sql",
  source: "select 1",
  cell_order: 0,
  execution_count: null,
  last_output: null,
  last_execution_time_ms: null,
  sql_target: null,
};

const engines: EngineOption[] = [
  { id: "e1", name: "trino", engine: "trino", enabled: true },
];

// Drive the underlying Radix Select via its hidden native <select> (rendered
// because the editor is mounted inside a <form>); the portal is unreliable in jsdom.
function selectOption(value: string) {
  const native = Array.from(
    document.querySelectorAll<HTMLSelectElement>("select"),
  ).find((s) => Array.from(s.options).some((o) => o.value === value));
  if (!native) throw new Error(`option ${value} not found`);
  fireEvent.change(native, { target: { value } });
}

function renderCell(overrides: Partial<ComponentProps<typeof CellEditor>> = {}) {
  const onChangeSqlTarget = vi.fn();
  render(
    <form>
      <CellEditor
        cell={sqlCell}
        onChangeSource={vi.fn()}
        onChangeType={vi.fn()}
        onChangeSqlTarget={onChangeSqlTarget}
        engines={engines}
        onDelete={vi.fn()}
        onMoveUp={vi.fn()}
        onMoveDown={vi.fn()}
        {...overrides}
      />
    </form>,
  );
  return { onChangeSqlTarget };
}

describe("CellEditor SQL target dropdown", () => {
  it("renders the SQL-target control defaulting to Daft with the engine option available", () => {
    renderCell();
    const trigger = screen.getByLabelText("SQL target");
    expect(trigger).toHaveTextContent(/Daft/);
    // The engine option is present in the hidden native select.
    const native = Array.from(
      document.querySelectorAll<HTMLSelectElement>("select"),
    ).find((s) => Array.from(s.options).some((o) => o.value === "trino"));
    expect(native).toBeTruthy();
  });

  it("selecting an engine calls onChangeSqlTarget with the engine name", () => {
    const { onChangeSqlTarget } = renderCell();
    selectOption("trino");
    expect(onChangeSqlTarget).toHaveBeenCalledWith("trino");
  });

  it("does not render the SQL-target control for a non-SQL cell", () => {
    renderCell({ cell: { ...sqlCell, cell_type: "code" } });
    expect(screen.queryByLabelText("SQL target")).not.toBeInTheDocument();
  });
});
