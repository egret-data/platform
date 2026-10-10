import { vi } from "vitest";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";

/**
 * A context with every callback a spy and nothing loaded, for rendering one
 * tab on its own. The tab under test reads only what the test overrides.
 */
export function makeEditorContext(overrides: Partial<EditorContext> = {}): EditorContext {
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
