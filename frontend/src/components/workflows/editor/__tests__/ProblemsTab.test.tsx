import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ProblemsBadge, ProblemsTab } from "@/components/workflows/editor/ProblemsTab";
import { makeEditorContext } from "./fixtures";

const SOURCE = [
  "id: hello",
  "namespace: company.team",
  "tasks:",
  "  - id: first",
  "    type: io.kestra.plugin.core.log.Log",
  "    message: hi",
  "",
].join("\n");

const items = () => within(screen.getByRole("list", { name: "Problems" })).getAllByRole("button");

describe("ProblemsTab", () => {
  it("while validating, shows a skeleton row that says so", () => {
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      validation: { status: "validating", findings: [], error: null, retry: vi.fn() },
    })} />);
    const row = screen.getByRole("status");
    expect(row).toHaveTextContent("Validating…");
    expect(row.querySelector("[data-slot=skeleton]")).not.toBeNull();
    expect(screen.queryByText("No problems")).toBeNull();
  });

  it("with nothing found, says there are no problems", () => {
    render(<ProblemsTab ctx={makeEditorContext({ source: SOURCE })} />);
    expect(screen.getByText("No problems")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Problems" })).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("for a read-only viewer, says only the schema was checked", () => {
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      readOnly: true,
      validation: { status: "idle", findings: [], error: null, retry: vi.fn() },
    })} />);
    expect(screen.getByText("No problems")).toBeInTheDocument();
    expect(screen.getByText(/Only the schema is checked here/)).toBeInTheDocument();
  });

  it("a failed validation shows the error with a retry, never 'No problems'", async () => {
    const retry = vi.fn();
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      validation: { status: "error", findings: [], error: "Failed to fetch", retry },
    })} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Validation failed");
    expect(alert).toHaveTextContent("Failed to fetch");
    expect(screen.queryByText("No problems")).toBeNull();
    await userEvent.click(within(alert).getByRole("button", { name: "Retry validation" }));
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it("still lists the schema's markers when validation failed", () => {
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      validation: { status: "error", findings: [], error: "boom", retry: vi.fn() },
      schemaMarkers: [{ severity: "error", message: "Missing property \"x\".", range: { startLine: 4, startCol: 5, endLine: 4, endCol: 7 } }],
    })} />);
    expect(items()).toHaveLength(1);
    expect(items()[0]).toHaveTextContent('Missing property "x".');
  });

  it("lists errors before warnings, each with its severity, message and path", () => {
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      validation: {
        status: "ok",
        error: null,
        retry: vi.fn(),
        findings: [
          { severity: "warning", code: "w", message: "namespace is odd", path: "namespace" },
          { severity: "error", code: "e", message: "message is wrong", path: "tasks.first.message" },
        ],
      },
      schemaMarkers: [
        { severity: "warning", message: "schema warning", range: { startLine: 1, startCol: 1, endLine: 1, endCol: 3 } },
        { severity: "error", message: "schema error", range: { startLine: 5, startCol: 5, endLine: 5, endCol: 9 } },
      ],
    })} />);
    const text = items().map((b) => b.textContent);
    expect(text).toEqual([
      "Error schema error Line 5",
      "Error message is wrong tasks.first.message Line 6",
      "Warning schema warning Line 1",
      "Warning namespace is odd namespace Line 2",
    ]);
  });

  it("selecting an item moves the editor to the finding's mapped range", async () => {
    const focusRange = vi.fn();
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      focusRange,
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [{ severity: "error", code: "e", message: "message is wrong", path: "tasks.first.message" }],
      },
    })} />);
    await userEvent.click(items()[0]);
    // "    message: hi" — the key at column 5, the value ending before column 16.
    expect(focusRange).toHaveBeenCalledWith({ startLine: 6, startCol: 5, endLine: 6, endCol: 16 });
  });

  it("works from the keyboard", async () => {
    const focusRange = vi.fn();
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      focusRange,
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [{ severity: "error", code: "e", message: "bad id", path: "id" }],
      },
    })} />);
    items()[0].focus();
    await userEvent.keyboard("{Enter}");
    expect(focusRange).toHaveBeenCalledWith({ startLine: 1, startCol: 1, endLine: 1, endCol: 10 });
  });

  it("lists an unmappable path with its path and focuses line 1", async () => {
    const focusRange = vi.fn();
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      focusRange,
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [{ severity: "error", code: "e", message: "no such task", path: "tasks.ghost.retry" }],
      },
    })} />);
    expect(items()[0]).toHaveTextContent("Error no such task tasks.ghost.retry (not found in the source, shown on line 1)");
    await userEvent.click(items()[0]);
    expect(focusRange).toHaveBeenCalledWith({ startLine: 1, startCol: 1, endLine: 1, endCol: 10 });
  });

  it("a parse error is listed at the line and column the server gave", async () => {
    const focusRange = vi.fn();
    render(<ProblemsTab ctx={makeEditorContext({
      source: SOURCE,
      focusRange,
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [{ severity: "error", code: "parse_error", message: "bad yaml (line 5, column 9)", path: "", at: { line: 5, col: 9 } }],
      },
    })} />);
    expect(items()[0]).toHaveTextContent("Line 5, column 9");
    await userEvent.click(items()[0]);
    expect(focusRange).toHaveBeenCalledWith({ startLine: 5, startCol: 9, endLine: 5, endCol: 10 });
  });
});

describe("ProblemsBadge", () => {
  it("counts the errors of both sources, in words for a screen reader, and hides at zero", () => {
    const ctx = makeEditorContext({
      source: SOURCE,
      validation: {
        status: "ok", error: null, retry: vi.fn(),
        findings: [
          { severity: "error", code: "e", message: "a", path: "id" },
          { severity: "warning", code: "w", message: "b", path: "id" },
        ],
      },
      schemaMarkers: [{ severity: "error", message: "c", range: { startLine: 1, startCol: 1, endLine: 1, endCol: 2 } }],
    });
    const { container, rerender } = render(<ProblemsBadge ctx={ctx} />);
    expect(container).toHaveTextContent("2, 2 errors");
    rerender(<ProblemsBadge ctx={{ ...ctx, schemaMarkers: [] }} />);
    expect(container).toHaveTextContent("1, 1 error");
    rerender(<ProblemsBadge ctx={makeEditorContext({ source: SOURCE })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
