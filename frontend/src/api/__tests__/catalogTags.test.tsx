/**
 * `useAssignResourceTag` must turn migration 0177's refusal into a react-query
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

import { ApiError } from "../client";
import { useAssignResourceTag, useUnassignResourceTag } from "../catalogTags";

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

    // The table-level sentinel, which 0177 deliberately still allows.
    result.current.mutate({ schema: "sales", table: "orders", column: "", tag_id: "t1" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.error).toBeNull();
  });
});


/**
 * `useUnassignResourceTag` — the same middle link, for the REMOVE half (SP4d debt
 * Task 11). Three links again:
 *
 * 1. `ApiError.from` reads the body's `error`/`code` — `client.test.ts`.
 * 2. **the `mutationFn` lets that `ApiError` propagate, with its `status` intact, so
 *    react-query exposes it as `error`** — here, real hook, real `apiJson`, stubbed
 *    `fetch`.
 * 3. `CatalogTagEditor` renders it (naming the permission on 403) —
 *    `components/access/__tests__/CatalogTagEditor.test.tsx`.
 *
 * Why this file and that one both exist, measured rather than assumed: that file
 * replaces this hook with a literal, so an `apiJson(...).catch(() => {})` inside this
 * `mutationFn` would leave all six of its assertions green while making the alert
 * unreachable in the browser. Only a test that drives the real hook can fail on it.
 *
 * `status` is asserted and not just `message`, because the component branches on it:
 * a `mutationFn` that rethrew `new Error(e.message)` would keep the message and lose
 * the 403, and the alert would fall through to the bare word "forbidden".
 *
 * ⚠ Both arms deliberately answer with a JSON BODY. `apiJson` calls `res.json()`
 * unconditionally, so a 204 No Content from `unassign_resource_tag` would reject on an
 * empty body and report a *successful* removal as a failure. The handler returns
 * `Json({"ok": true})` today (`crates/egret-bi/src/catalog_tags.rs`), so that is the
 * shape under test; it is noted because the hazard is one `StatusCode::NO_CONTENT`
 * away and no test here would notice.
 */
describe("useUnassignResourceTag", () => {
  it("surfaces a 403 as a failure that keeps ApiError's status, not just its message", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      // `Error::Forbidden`'s Display is the whole body: there is no per-permission
      // sentence to forward, which is why the component names the permission itself.
      new Response(JSON.stringify({ error: "forbidden" }), { status: 403 }),
    );
    const { result } = renderHook(() => useUnassignResourceTag("wh1"), { wrapper });

    result.current.mutate({ schema: "sales", table: "orders", column: "", tag_id: "t1" });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error).toBeInstanceOf(ApiError);
    expect((result.current.error as ApiError).status).toBe(403);
    expect(result.current.error?.message).toBe("forbidden");
  });

  it("surfaces a 500's own sentence verbatim", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ error: "recompiling the bound policy profiles failed" }), {
        status: 500,
      }),
    );
    const { result } = renderHook(() => useUnassignResourceTag("wh1"), { wrapper });

    result.current.mutate({ schema: "sales", table: "orders", column: "", tag_id: "t1" });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect((result.current.error as ApiError).status).toBe(500);
    expect(result.current.error?.message).toBe("recompiling the bound policy profiles failed");
  });

  it("reports no error for a removal the backend accepts", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    const { result } = renderHook(() => useUnassignResourceTag("wh1"), { wrapper });

    result.current.mutate({ schema: "sales", table: "orders", column: "", tag_id: "t1" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.error).toBeNull();
  });

  it("sends DELETE, so a mutationFn copied from the assign half cannot pass", async () => {
    const f = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const { result } = renderHook(() => useUnassignResourceTag("wh1"), { wrapper });

    result.current.mutate({ schema: "sales", table: "orders", column: "email", tag_id: "t1" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/api/control-plane/warehouses/wh1/resource-tags");
    expect(init.method).toBe("DELETE");
    expect(JSON.parse(String(init.body))).toEqual({
      schema: "sales",
      table: "orders",
      column: "email",
      tag_id: "t1",
    });
  });
});
