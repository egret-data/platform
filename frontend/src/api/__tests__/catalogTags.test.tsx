/**
 * `useAssignResourceTag` must turn migration 0166's refusal into a react-query
 * FAILURE whose `error.message` is the backend's own sentence — SP4d debt Task 3,
 * fix round, Minor 1.
 *
 * This is the middle link of a three-link chain, and it is the one nothing covered:
 *
 * 1. `ApiError.from` puts the body's `error` field on `Error.message` — pinned by
 *    `client.test.ts`'s "ApiError carries the response body's code".
 * 2. **the hook's `mutationFn` lets that `ApiError` propagate, so react-query
 *    exposes it as `error`** — here, with the real hook and a real `apiJson` over a
 *    stubbed `fetch`.
 * 3. `CatalogTagEditor` renders `assign.error.message` — pinned by
 *    `components/access/__tests__/CatalogTagEditor.test.tsx`.
 *
 * Link 2 is where an `await apiJson(...).catch(() => {})` or a `mutationFn` that
 * swallowed non-2xx would make the component's alert unreachable while its own test
 * stayed green, because that test mocks this hook.
 */
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useAssignResourceTag } from "../catalogTags";

/** The exact sentence `unresolvable_coordinate` produces for a wildcarded column. */
const BACKEND_MESSAGE =
  "column must not contain `*` or `?`: the engine would match it as a wildcard and egret could not resolve the tag";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useAssignResourceTag", () => {
  it("surfaces the 400's message verbatim as the mutation's error", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ error: BACKEND_MESSAGE }), { status: 400 }),
    );
    const { result } = renderHook(() => useAssignResourceTag("wh1"), { wrapper });

    result.current.mutate({ schema: "sales", table: "orders", column: "ema*", tag_id: "t1" });

    await waitFor(() => expect(result.current.isError).toBe(true));
    // Verbatim, because the level name is the actionable half and the UI must not
    // paraphrase it.
    expect(result.current.error?.message).toBe(BACKEND_MESSAGE);
  });

  it("does not report an error for a coordinate the database accepts", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    const { result } = renderHook(() => useAssignResourceTag("wh1"), { wrapper });

    // The table-level sentinel, which 0166 deliberately still allows.
    result.current.mutate({ schema: "sales", table: "orders", column: "", tag_id: "t1" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.error).toBeNull();
  });
});
