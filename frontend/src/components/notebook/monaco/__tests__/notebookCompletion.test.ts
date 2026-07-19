import { afterEach, describe, expect, it, vi } from "vitest";
import {
  registerNotebookCompletions,
  setNotebookCompletionContext,
} from "../notebookCompletion";

function fakeMonaco() {
  const providers: Record<string, any> = {};
  return {
    providers,
    languages: {
      CompletionItemKind: { Function: 1, Variable: 2, Keyword: 3, Field: 4 },
      CompletionItemInsertTextRule: { InsertAsSnippet: 5 },
      registerCompletionItemProvider: (lang: string, provider: any) => {
        providers[lang] = provider;
        return { dispose: vi.fn() };
      },
    },
  };
}

// Minimal fake Monaco model — enough for both providers' code paths
// (getOffsetAt/getPositionAt for python; getWordUntilPosition for sql).
function fakeModel(value = "df.") {
  return {
    getValue: () => value,
    getOffsetAt: () => value.length,
    getPositionAt: (offset: number) => ({ lineNumber: 1, column: offset + 1 }),
    getWordUntilPosition: () => ({ startColumn: 1, endColumn: 1 }),
  };
}

const fakePosition = { lineNumber: 1, column: 1 };

// `registerNotebookCompletions` registers ONCE globally (module-level
// `registered` flag, matching the real Monaco constraint of one provider per
// language) — so all tests in this file share a single fake-monaco instance
// and its captured providers, mirroring how CellEditor calls it exactly once
// per page load. Context (kernel/sqlWords) is what varies per test.
const m = fakeMonaco();
registerNotebookCompletions(m as any);

describe("registerNotebookCompletions", () => {
  afterEach(() => {
    setNotebookCompletionContext(null);
  });

  it("registers python and sql exactly once (idempotent)", () => {
    const before = { ...m.providers };
    registerNotebookCompletions(m as any); // second call must be a no-op
    expect(m.providers).toEqual(before);
    expect(Object.keys(m.providers).sort()).toEqual(["python", "sql"]);
  });

  it("SQL-Lab-safe: with no context, python returns only statics and sql returns only keywords", async () => {
    setNotebookCompletionContext(null);

    const pyResult = await m.providers.python.provideCompletionItems(fakeModel(), fakePosition);
    const pyLabels = pyResult.suggestions.map((s: any) => s.label);
    expect(pyLabels).toEqual(expect.arrayContaining(["trino", "starrocks", "display"]));
    expect(pyLabels).not.toEqual(expect.arrayContaining(["df.a"]));

    const sqlResult = m.providers.sql.provideCompletionItems(fakeModel(), fakePosition);
    const sqlLabels = sqlResult.suggestions.map((s: any) => s.label);
    expect(sqlLabels).toEqual(expect.arrayContaining(["SELECT", "FROM", "WHERE"]));
    expect(sqlLabels).not.toEqual(expect.arrayContaining(["orders"]));
  });

  it("with a notebook context, python suggestions include kernel matches and sql includes schema words", async () => {
    setNotebookCompletionContext({
      kernel: {
        complete: async () => ({ matches: ["df.a", "df.b"], cursorStart: 0, cursorEnd: 3 }),
      } as any,
      sqlWords: () => ["orders", "user_id"],
    });

    const pyResult = await m.providers.python.provideCompletionItems(fakeModel("df."), fakePosition);
    const pyLabels = pyResult.suggestions.map((s: any) => s.label);
    expect(pyLabels).toEqual(expect.arrayContaining(["df.a", "df.b"]));

    const sqlResult = m.providers.sql.provideCompletionItems(fakeModel(), fakePosition);
    const sqlLabels = sqlResult.suggestions.map((s: any) => s.label);
    expect(sqlLabels).toEqual(expect.arrayContaining(["orders", "user_id"]));
  });

  it("reverts to SQL-Lab-safe behavior once the context is cleared", async () => {
    setNotebookCompletionContext({
      kernel: { complete: async () => ({ matches: ["df.a"], cursorStart: 0, cursorEnd: 3 }) } as any,
      sqlWords: () => ["orders"],
    });
    setNotebookCompletionContext(null);

    const sqlResult = m.providers.sql.provideCompletionItems(fakeModel(), fakePosition);
    const sqlLabels = sqlResult.suggestions.map((s: any) => s.label);
    expect(sqlLabels).not.toEqual(expect.arrayContaining(["orders"]));
  });
});
