import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { CellEditor } from "../CellEditor";
import type { Cell } from "@/api/notebooks";

// A minimal fake Monaco surface: enough for CellEditor's handleMount to
// register commands/focus hooks (S1-3) without pulling in real Monaco (jsdom
// can't load it). `monacoStub` is hoisted so per-test code can drive the
// captured onDidFocusEditorText listener / addCommand callbacks directly.
const monacoStub = vi.hoisted(() => {
  const KeyMod = { Shift: 1 << 10, CtrlCmd: 1 << 11, Alt: 1 << 9 };
  const KeyCode = { Enter: 3, Escape: 9 };
  return {
    KeyMod,
    KeyCode,
    commands: {} as Record<number, () => void>,
    focusListener: undefined as (() => void) | undefined,
    domBlurred: false,
    reset() {
      this.commands = {};
      this.focusListener = undefined;
      this.domBlurred = false;
    },
  };
});

vi.mock("@monaco-editor/react", () => ({
  default: ({
    value,
    onChange,
    onMount,
  }: {
    value: string;
    onChange?: (v?: string) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
  }) => {
    const editor = {
      focus: () => monacoStub.focusListener?.(),
      getDomNode: () => ({
        blur: () => {
          monacoStub.domBlurred = true;
        },
      }),
      onDidFocusEditorText: (cb: () => void) => {
        monacoStub.focusListener = cb;
      },
      onDidContentSizeChange: () => {},
      getContentHeight: () => 100,
      addCommand: (keybinding: number, cb: () => void) => {
        monacoStub.commands[keybinding] = cb;
      },
    };
    const monaco = {
      KeyMod: monacoStub.KeyMod,
      KeyCode: monacoStub.KeyCode,
      languages: {
        registerCompletionItemProvider: () => {},
        CompletionItemKind: { Function: 1, Keyword: 2 },
        CompletionItemInsertTextRule: { InsertAsSnippet: 4 },
      },
    };
    // Real Monaco fires onMount post-mount (once), not during render — mirror
    // that here (via useEffect, [] deps) so (a) handleMount's setEditorHeight()
    // doesn't trigger React's "setState while rendering a different component"
    // warning, and (b) a re-render caused by that very setState doesn't wipe
    // monacoStub's captured commands/listener before the test can use them.
    useEffect(() => {
      monacoStub.reset();
      onMount?.(editor, monaco);
    }, []);
    return <textarea aria-label="code" value={value} onChange={(e) => onChange?.(e.target.value)} />;
  },
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

  it("lets you type a multi-char result variable (local state) and commits on blur", () => {
    const onChangeSqlInto = vi.fn();
    render(
      <CellEditor
        {...baseProps}
        onChangeSqlInto={onChangeSqlInto}
        engines={[{ id: "e1", name: "Trino", engine: "trino", enabled: true }]}
        cell={cell({ id: "s", cell_type: "sql", source: "SELECT 1", cell_order: 0, sql_target: "e1", sql_into: "df" })}
      />,
    );
    const input = screen.getByLabelText(/result variable/i);
    // Typing updates local state (server state hasn't changed, so no revert).
    fireEvent.change(input, { target: { value: "sales_df" } });
    expect(input).toHaveValue("sales_df");
    // No premature per-keystroke commit.
    expect(onChangeSqlInto).not.toHaveBeenCalled();
    // Commit happens on blur.
    fireEvent.blur(input);
    expect(onChangeSqlInto).toHaveBeenCalledWith("sales_df");
  });

  it("offers a Daft-local target option for SQL cells", () => {
    render(
      <CellEditor
        {...baseProps}
        engines={[{ id: "e1", name: "Trino", engine: "trino", enabled: true }]}
        cell={cell({ id: "s", cell_type: "sql", source: "SELECT 1", cell_order: 0, sql_target: "daft", sql_into: "df" })}
      />,
    );
    expect(screen.getByLabelText("SQL target")).toHaveTextContent(/daft/i);
  });

  // ── Active cell + keyboard command wiring (S1-3) ─────────────────────────

  describe("active cell + keyboard commands", () => {
    const codeCell = cell({ id: "c1", cell_type: "code", source: "1 + 1", cell_order: 0 });

    it("shows an active ring / data-active when active", () => {
      const { container } = render(<CellEditor {...baseProps} cell={codeCell} active />);
      const root = container.querySelector("[data-active]");
      expect(root).toHaveAttribute("data-active", "true");
      expect(root?.className).toMatch(/ring-2/);
    });

    it("is not marked active by default", () => {
      const { container } = render(<CellEditor {...baseProps} cell={codeCell} />);
      expect(container.querySelector("[data-active]")).toHaveAttribute("data-active", "false");
    });

    it("clicking non-editor chrome (the gutter) calls onSelectCell, not onFocusCell", () => {
      const onSelectCell = vi.fn();
      const onFocusCell = vi.fn();
      render(
        <CellEditor
          {...baseProps}
          cell={codeCell}
          onSelectCell={onSelectCell}
          onFocusCell={onFocusCell}
        />,
      );
      fireEvent.click(screen.getByText("In [ ]:"));
      expect(onSelectCell).toHaveBeenCalledTimes(1);
      expect(onFocusCell).not.toHaveBeenCalled();
    });

    it("registers a focus function and reports editor focus via onFocusCell", () => {
      const registerFocus = vi.fn();
      const onFocusCell = vi.fn();
      render(
        <CellEditor
          {...baseProps}
          cell={codeCell}
          registerFocus={registerFocus}
          onFocusCell={onFocusCell}
        />,
      );
      expect(registerFocus).toHaveBeenCalledWith(expect.any(Function));
      // Simulate Monaco reporting that its text editor gained focus.
      monacoStub.focusListener?.();
      expect(onFocusCell).toHaveBeenCalledTimes(1);
    });

    it("wires Shift+Enter / Ctrl+Enter / Alt+Enter / Escape to the right callbacks", () => {
      const onRun = vi.fn();
      const onRunAndNext = vi.fn();
      const onRunAndBelow = vi.fn();
      const onEnterCommandMode = vi.fn();
      render(
        <CellEditor
          {...baseProps}
          cell={codeCell}
          onRun={onRun}
          onRunAndNext={onRunAndNext}
          onRunAndBelow={onRunAndBelow}
          onEnterCommandMode={onEnterCommandMode}
        />,
      );
      const { KeyMod, KeyCode, commands } = monacoStub;

      commands[KeyMod.Shift | KeyCode.Enter]?.();
      expect(onRunAndNext).toHaveBeenCalledTimes(1);
      expect(onRun).not.toHaveBeenCalled();

      commands[KeyMod.CtrlCmd | KeyCode.Enter]?.();
      expect(onRun).toHaveBeenCalledTimes(1);

      commands[KeyMod.Alt | KeyCode.Enter]?.();
      expect(onRunAndBelow).toHaveBeenCalledTimes(1);

      commands[KeyCode.Escape]?.();
      expect(onEnterCommandMode).toHaveBeenCalledTimes(1);
      expect(monacoStub.domBlurred).toBe(true);
    });
  });
});
