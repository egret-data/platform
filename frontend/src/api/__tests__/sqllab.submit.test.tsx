import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";
import { useSubmitQuery } from "@/api/sqllab";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

function respond(status: number, body: string) {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(body, { status })));
}

/** KI-086: a refusal before the query starts reaches the user as its message, not as raw JSON. */
describe("useSubmitQuery — a refused submit", () => {
  afterEach(() => vi.unstubAllGlobals());

  async function failure(): Promise<Error> {
    const { result } = renderHook(() => useSubmitQuery("eng"), { wrapper });
    let err: unknown;
    await act(async () => {
      await result.current.mutateAsync({ database_id: "db1", sql: "SELECT {{n}}" }).catch((e) => { err = e; });
    });
    return err as Error;
  }

  it("carries the server's error text, not the JSON body", async () => {
    respond(400, JSON.stringify({ error: "invalid input: parameter 'n' is not a number: abc" }));
    expect((await failure()).message).toBe("invalid input: parameter 'n' is not a number: abc");
  });

  it("falls back to the body text, then to the status, when the body is not an error object", async () => {
    respond(502, "bad gateway");
    expect((await failure()).message).toBe("bad gateway");
    respond(500, "");
    expect((await failure()).message).toBe("The query could not be started (HTTP 500).");
  });
});
