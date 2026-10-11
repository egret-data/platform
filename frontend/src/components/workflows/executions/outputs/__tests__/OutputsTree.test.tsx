import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { TaskRunDto } from "@/api/generated/model";
import { OutputsTree } from "@/components/workflows/executions/outputs/OutputsTree";
import { EXEC, FileServer, uri, WS } from "../../files/__tests__/fileServer";

/**
 * The outputs tree of one task run, rendered for real (FE-TEST-1); the file
 * rows it carries read their size from the file routes served from memory.
 */
function taskRun(outputs: Record<string, unknown>, extra: Partial<TaskRunDto> = {}): TaskRunDto {
  return {
    id: "r-1",
    execution_id: EXEC,
    run_ix: 1,
    task_ix: 0,
    task_id: "t1",
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    attempts: [],
    outputs,
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:01.000001Z",
    ...extra,
  };
}

const OUTPUTS = {
  key: "hello",
  count: 3,
  rows: [{ id: 1 }, { id: 2 }],
  meta: { deep: { x: 1 } },
  uri: uri("out.csv"),
};

let writeText: ReturnType<typeof vi.fn>;

beforeEach(() => {
  writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function renderTree(run: TaskRunDto) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <OutputsTree ws={WS} executionId={EXEC} run={run} />
    </QueryClientProvider>,
  );
}

const items = () => within(screen.getByRole("tree", { name: "Outputs of t1" })).getAllByRole("treeitem");
const labels = () => items().map((i) => i.getAttribute("aria-label"));
const item = (name: string) => within(screen.getByRole("tree")).getByRole("treeitem", { name: new RegExp(`^${name.replace(/[[\]]/g, "\\$&")},`) });

describe("OutputsTree", () => {
  it("shows the top level with its small containers open and the deeper ones closed", () => {
    new FileServer().file(uri("out.csv"), 10).install();
    renderTree(taskRun(OUTPUTS));
    expect(labels()).toEqual([
      'key, "hello"',
      "count, 3",
      "rows, 2 items",
      "[0], 1 key",
      "[1], 1 key",
      "meta, 1 key",
      "deep, 1 key",
      "uri, file out.csv",
    ]);
    expect(item("rows")).toHaveAttribute("aria-expanded", "true");
    expect(item("deep")).toHaveAttribute("aria-expanded", "false");
    // A leaf is not expandable at all.
    expect(item("key")).not.toHaveAttribute("aria-expanded");
    expect(item("deep")).toHaveAttribute("aria-level", "2");
  });

  it("expands and collapses a container by click", async () => {
    new FileServer().file(uri("out.csv"), 10).install();
    renderTree(taskRun(OUTPUTS));
    await userEvent.click(item("deep"));
    expect(item("deep")).toHaveAttribute("aria-expanded", "true");
    expect(labels()).toContain("x, 1");
    await userEvent.click(item("rows"));
    expect(item("rows")).toHaveAttribute("aria-expanded", "false");
    expect(labels()).not.toContain("[0], 1 key");
  });

  it("moves, expands and collapses from the keyboard, with one tab stop", async () => {
    new FileServer().file(uri("out.csv"), 10).install();
    renderTree(taskRun(OUTPUTS));
    // One item is in the tab order: the first.
    expect(items().filter((i) => i.tabIndex === 0)).toEqual([item("key")]);

    await userEvent.tab();
    expect(item("key")).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}");
    expect(item("rows")).toHaveFocus();
    // Left on an open container closes it; Right opens it, then enters it.
    await userEvent.keyboard("{ArrowLeft}");
    expect(item("rows")).toHaveAttribute("aria-expanded", "false");
    await userEvent.keyboard("{ArrowRight}");
    expect(item("rows")).toHaveAttribute("aria-expanded", "true");
    expect(item("rows")).toHaveFocus();
    await userEvent.keyboard("{ArrowRight}");
    expect(item("[0]")).toHaveFocus();
    // Left on a child goes to its parent.
    await userEvent.keyboard("{ArrowLeft}");
    expect(item("rows")).toHaveFocus();
    await userEvent.keyboard("{End}");
    expect(item("uri")).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    expect(item("deep")).toHaveFocus();
    // Enter toggles a container.
    await userEvent.keyboard("{Enter}");
    expect(item("deep")).toHaveAttribute("aria-expanded", "true");
    await userEvent.keyboard("{ArrowDown}");
    expect(item("x")).toHaveFocus();
    await userEvent.keyboard("{Home}");
    expect(item("key")).toHaveFocus();
    // The focused item is the selected one, and the only tab stop.
    expect(item("key")).toHaveAttribute("aria-selected", "true");
    expect(items().filter((i) => i.tabIndex === 0)).toEqual([item("key")]);
  });

  it("copies the selected value: a string as itself, anything else as JSON", async () => {
    new FileServer().file(uri("out.csv"), 10).install();
    renderTree(taskRun(OUTPUTS));
    await userEvent.click(item("key"));
    await userEvent.click(screen.getByRole("button", { name: "Copy value" }));
    expect(writeText).toHaveBeenLastCalledWith("hello");
    expect(await screen.findByText("Copied the value.")).toBeInTheDocument();

    await userEvent.click(item("[1]"));
    await userEvent.click(screen.getByRole("button", { name: "Copy value" }));
    expect(writeText).toHaveBeenLastCalledWith('{\n  "id": 2\n}');
  });

  it("copies the selected path as an expression", async () => {
    new FileServer().file(uri("out.csv"), 10).install();
    renderTree(taskRun(OUTPUTS));
    await userEvent.click(item("deep"));
    expect(screen.getByTestId("output-expression")).toHaveTextContent("{{ outputs.t1.meta.deep }}");
    await userEvent.click(screen.getByRole("button", { name: "Copy expression" }));
    expect(writeText).toHaveBeenLastCalledWith("{{ outputs.t1.meta.deep }}");
    expect(await screen.findByText("Copied the expression.")).toBeInTheDocument();

    await userEvent.click(item("[0]"));
    await userEvent.click(screen.getByRole("button", { name: "Copy expression" }));
    expect(writeText).toHaveBeenLastCalledWith("{{ outputs.t1.rows[0] }}");
    // No loop note for a run without a value.
    expect(screen.queryByText(/reached through its value/)).toBeNull();
  });

  it("writes a loop run's paths in Kestra's outputs.<taskId>[value] form, and says so", async () => {
    renderTree(taskRun({ key: "v" }, { value: "eu-west" }));
    await userEvent.click(item("key"));
    await userEvent.click(screen.getByRole("button", { name: "Copy expression" }));
    expect(writeText).toHaveBeenLastCalledWith('{{ outputs.t1["eu-west"].key }}');
    expect(screen.getByText(/reached through its value/)).toHaveTextContent('outputs.t1["eu-west"]');
  });

  it("says when the clipboard refused, and keeps the expression on screen to copy by hand", async () => {
    writeText.mockRejectedValue(new Error("denied"));
    renderTree(taskRun({ key: "v" }));
    await userEvent.click(item("key"));
    await userEvent.click(screen.getByRole("button", { name: "Copy expression" }));
    expect(await screen.findByText(/could not be copied/)).toBeInTheDocument();
    expect(screen.getByTestId("output-expression")).toHaveTextContent("{{ outputs.t1.key }}");
  });

  it("lists every file in the outputs as a file row, nested ones included", async () => {
    const server = new FileServer().file(uri("out.csv"), 2048).file(uri("deep.ion"), 10).install();
    renderTree(taskRun({ ...OUTPUTS, meta: { deep: { x: 1, f: uri("deep.ion") } } }));
    const files = screen.getByRole("region", { name: "Files" });
    expect(await within(files).findByText("2.0 KiB")).toBeInTheDocument();
    expect(within(files).getByRole("link", { name: "Download deep.ion" })).toBeInTheDocument();
    expect(within(files).getByText("{{ outputs.t1.meta.deep.f }}")).toBeInTheDocument();
    expect(server.callsTo("metas").map((c) => c.query.get("path")).sort()).toEqual([uri("deep.ion"), uri("out.csv")].sort());
  });

  it("says a run with no outputs has none", () => {
    renderTree(taskRun({}));
    expect(screen.getByText("This task run has no outputs.")).toBeInTheDocument();
    expect(screen.queryByRole("tree")).toBeNull();
  });
});
