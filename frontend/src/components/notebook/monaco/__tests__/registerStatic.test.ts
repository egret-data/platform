import { describe, it, expect, vi } from "vitest";
import { registerStaticCompletions } from "../registerStatic";

function fakeMonaco() {
  const registered: string[] = [];
  return {
    registered,
    languages: {
      CompletionItemKind: { Function: 1, Variable: 2, Keyword: 3 },
      CompletionItemInsertTextRule: { InsertAsSnippet: 4 },
      registerCompletionItemProvider: (lang: string) => {
        registered.push(lang);
        return { dispose: vi.fn() };
      },
    },
  };
}

describe("registerStaticCompletions", () => {
  it("registers python and sql once (idempotent)", () => {
    const m = fakeMonaco();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerStaticCompletions(m as any);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerStaticCompletions(m as any);
    expect(m.registered.sort()).toEqual(["python", "sql"]);
  });
});
