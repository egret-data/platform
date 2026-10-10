import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { DocsTab } from "@/components/workflows/editor/DocsTab";
import type { FlowSchema } from "@/components/workflows/editor/flowSchema";
import { makeEditorContext } from "./fixtures";

/**
 * The served shape (`spec/schema/mod.rs`): type unions under `$defs`, each
 * type's spellings as `const`s, `required` split into `id`/`type` and
 * `then.required`, outputs as `x-egret-outputs`. Two task types with
 * different properties, so documenting the wrong one cannot pass.
 */
const GUARD = { required: ["type"], properties: { type: { type: "string" } } };
const SCHEMA: FlowSchema = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  title: "An egret flow",
  type: "object",
  required: ["id", "namespace", "tasks"],
  properties: {
    id: { type: "string", title: "The flow's id." },
    namespace: { type: "string", title: "The flow's namespace." },
    labels: { type: "object", title: "Labels." },
    tasks: { type: "array", items: { $ref: "#/$defs/egret.task" }, minItems: 1, title: "The tasks." },
  },
  $defs: {
    "egret.task": {
      oneOf: [{ $ref: "#/$defs/io.kestra.plugin.core.log.Log" }, { $ref: "#/$defs/egret.sql" }],
    },
    "egret.trigger": { oneOf: [{ $ref: "#/$defs/io.kestra.plugin.core.trigger.Schedule" }] },
    "io.kestra.plugin.core.log.Log": {
      title: "Emit log entries from a flow.",
      markdownDescription: "Render **one or many** messages.",
      type: "object",
      required: ["id", "type"],
      if: GUARD,
      then: { required: ["message"] },
      properties: {
        id: { type: "string" },
        type: {
          anyOf: [
            { const: "io.kestra.plugin.core.log.Log", description: "Kestra's `io.kestra.plugin.core.log.Log`." },
            { const: "core.log", description: "Alias of `io.kestra.plugin.core.log.Log`." },
          ],
        },
        level: { type: "string", enum: ["ERROR", "INFO"], default: "INFO", title: "The level to log at." },
        message: {
          anyOf: [{ type: "string" }, { type: "array", items: { type: "string" } }],
          title: "One or more messages.",
        },
        retry: {
          anyOf: [{ allOf: [{ $ref: "#/$defs/io.kestra.core.models.tasks.retrys.Constant-2" }, { title: "Retry" }] }],
        },
      },
      "x-egret-outputs": [],
    },
    "egret.sql": {
      title: "Run one SQL statement.",
      type: "object",
      required: ["id", "type"],
      if: GUARD,
      then: { required: ["database", "sql"] },
      properties: {
        id: { type: "string" },
        type: { anyOf: [{ const: "egret.sql" }] },
        database: { type: "string", title: "The connection's name." },
        sql: { type: "string", title: "The statement." },
        fetch: { type: "string", enum: ["NONE", "FETCH"], default: "NONE", title: "What to fetch." },
      },
      "x-egret-outputs": [{ name: "rows", type: "array", description: "Every row, with `fetch: FETCH`." }],
    },
    "io.kestra.plugin.core.trigger.Schedule": {
      title: "Schedule a flow.",
      type: "object",
      required: ["id", "type"],
      if: GUARD,
      then: { required: ["cron"] },
      properties: {
        id: { type: "string" },
        type: { anyOf: [{ const: "io.kestra.plugin.core.trigger.Schedule" }, { const: "core.schedule" }] },
        cron: { type: "string", title: "The cron expression." },
      },
      "x-egret-outputs": [],
    },
    "io.kestra.core.models.tasks.retrys.Constant-2": { type: "object", properties: {} },
  },
};

const SOURCE = [
  "id: hello",
  "namespace: company.team",
  "tasks:",
  "  - id: say",
  "    type: core.log",
  "    message: hi",
  "  - id: query",
  "    type: egret.sql",
  "    database: dwh",
  "  - id: odd",
  "    type: com.example.Unknown",
  "triggers:",
  "  - id: daily",
  "    type: io.kestra.plugin.core.trigger.Schedule",
  "    cron: '0 9 * * *'",
  "",
].join("\n");
const at = (text: string) => SOURCE.indexOf(text) + 1;

const ready = { status: "ready" as const, schema: SCHEMA, etag: '"e1"', error: null, retry: vi.fn() };
const renderDocs = (cursorOffset: number | null) =>
  render(<DocsTab ctx={makeEditorContext({ source: SOURCE, cursorOffset, schema: ready })} />);

/** The properties table as `name → [type, required, default, description]`. */
function propertyRows(name = "Properties") {
  const table = screen.getByRole("table", { name });
  return Object.fromEntries(
    within(table).getAllByRole("row").slice(1).map((r) => {
      const cells = within(r).getAllByRole("cell").map((c) => c.textContent);
      return [cells[0], cells.slice(1)];
    }),
  );
}

describe("DocsTab", () => {
  it("documents the task under the cursor: description, properties and outputs", () => {
    renderDocs(at("database: dwh"));
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("egret.sql");
    expect(screen.getByText("Run one SQL statement.")).toBeInTheDocument();
    const rows = propertyRows();
    // Required ones first, each read from `required` AND `then.required`.
    expect(Object.keys(rows)).toEqual(["database", "id", "sql", "type", "fetch"]);
    expect(rows.database).toEqual(["string", "Yes", "", "The connection's name."]);
    expect(rows.fetch).toEqual(["string", "No", "NONE", "What to fetch."]);
    const outputs = propertyRows("Outputs");
    expect(outputs.rows).toEqual(["array", "Every row, with fetch: FETCH."]);
  });

  it("follows an alias to its type, says so, and renders the markdown description", () => {
    renderDocs(at("message: hi"));
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("io.kestra.plugin.core.log.Log");
    expect(screen.getByText(/written here as/)).toHaveTextContent("written here as core.log");
    expect(screen.getByText("one or many").tagName).toBe("STRONG");
    const rows = propertyRows();
    expect(rows.message).toEqual(["string | array", "Yes", "", "One or more messages."]);
    expect(rows.level).toEqual(["string", "No", "INFO", "The level to log at."]);
    expect(rows.retry[0]).toBe("Constant");
    expect(screen.getByText("This type has no documented outputs.")).toBeInTheDocument();
  });

  it("documents the trigger under the cursor", () => {
    renderDocs(at("cron:"));
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("io.kestra.plugin.core.trigger.Schedule");
    expect(propertyRows().cron).toEqual(["string", "Yes", "", "The cron expression."]);
  });

  it("falls back to the flow's own documentation outside any task", () => {
    renderDocs(at("company.team"));
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Flow");
    const rows = propertyRows();
    expect(Object.keys(rows)).toEqual(["id", "namespace", "tasks", "labels"]);
    expect(rows.tasks).toEqual(["array", "Yes", "", "The tasks."]);
    expect(screen.queryByRole("table", { name: "Outputs" })).toBeNull();
  });

  it("with no cursor yet, shows the flow's documentation", () => {
    renderDocs(null);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Flow");
  });

  it("names a type the schema does not know, then shows the flow's documentation", () => {
    renderDocs(at("type: com.example.Unknown") + 8);
    expect(screen.getByText(/No documentation for/)).toHaveTextContent(
      "No documentation for com.example.Unknown. Showing the flow's documentation.",
    );
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Flow");
  });

  it("while the schema loads, shows a skeleton", () => {
    render(<DocsTab ctx={makeEditorContext({
      source: SOURCE,
      schema: { status: "loading", schema: null, etag: null, error: null, retry: vi.fn() },
    })} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading the documentation");
    expect(document.querySelector("[data-slot=skeleton]")).not.toBeNull();
  });

  it("says 'Autocomplete unavailable' with a retry when the schema query fails", async () => {
    const retry = vi.fn();
    render(<DocsTab ctx={makeEditorContext({
      source: SOURCE,
      cursorOffset: at("database"),
      schema: { status: "error", schema: null, etag: null, error: "internal error", retry },
    })} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Autocomplete unavailable");
    expect(alert).toHaveTextContent("internal error");
    expect(screen.queryByRole("table")).toBeNull();
    await userEvent.click(within(alert).getByRole("button", { name: "Retry loading the schema" }));
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
