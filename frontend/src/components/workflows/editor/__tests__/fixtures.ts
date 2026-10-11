import { vi } from "vitest";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { collectProblems } from "@/components/workflows/editor/problems";
import { createYamlView, parseYaml } from "@/components/workflows/editor/yamlPath";

/**
 * A context with every callback a spy and nothing loaded, for rendering one
 * tab on its own. The tab under test reads only what the test overrides.
 */
export function makeEditorContext(overrides: Partial<EditorContext> = {}): EditorContext {
  const base = makeBase(overrides);
  // What `useFlowEditor` derives, derived the same way unless the test sets it.
  const yaml = overrides.yaml ?? createYamlView(parseYaml(base.source), null);
  const problems = overrides.problems ?? collectProblems(yaml, base.validation.findings, base.schemaMarkers);
  return { ...base, yaml, problems };
}

function makeBase(overrides: Partial<EditorContext>): Omit<EditorContext, "yaml" | "problems"> {
  return {
    workspace: "ws1",
    modelKey: "new",
    source: "",
    setSource: vi.fn(),
    readOnly: false,
    validation: { status: "ok", findings: [], error: null, retry: vi.fn() },
    schema: { status: "ready", schema: null, etag: null, error: null, retry: vi.fn() },
    schemaMarkers: [],
    setSchemaMarkers: vi.fn(),
    cursorOffset: null,
    setCursorOffset: vi.fn(),
    focusRange: vi.fn(),
    bridge: { attach: vi.fn(), detach: vi.fn(), focus: vi.fn() },
    ...overrides,
  };
}
