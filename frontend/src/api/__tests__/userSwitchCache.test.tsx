/**
 * SP2 final review F1: switching users in the same tab (logout → login,
 * or impersonation start/end) must not leave the previous identity's
 * `["my-workspaces"]` (or any other non-`["me"]`) query cached past its
 * `staleTime` — `HomeRedirect` reads straight from the cache and would
 * otherwise land the new user on the old user's workspace.
 *
 * Each hook here is exercised in isolation against a real `QueryClient`,
 * with `fetch` stubbed (same style as `src/__tests__/governanceRouting.test.tsx`)
 * rather than mocking `@/api/*` — the thing under test IS the cache-clearing
 * logic inside these hooks' `onSuccess`.
 */
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useExitImpersonation, useLogin, useLogout } from "@/api/hooks";
import { useStartPlatformImpersonation } from "@/api/impersonation";
import { clearUserScopedCache } from "@/api/queryClient";
import { loadRuleDraft, saveRuleDraft } from "@/routes/governance/ruleDraft";

const STALE_WORKSPACES = [{ id: "w1", slug: "w1", name: "Previous user's workspace", features: [] }];

function okResponse() {
  return Promise.resolve(new Response(null, { status: 204 }));
}

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }),
  );
}

/** Resolves on a real macrotask, not the current microtask queue — unlike
 * `jsonResponse`, a promise chain built on this cannot finish "by accident"
 * before an unawaited caller's `await mutateAsync()` continuation runs. */
function delayedJsonResponse(body: unknown) {
  return new Promise<Response>((resolve) => {
    setTimeout(
      () => resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } })),
      5,
    );
  });
}

function wrapper(qc: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("query cache is cleared on every user-identity switch (F1)", () => {
  it("logout removes the previous user's my-workspaces cache", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["my-workspaces"], STALE_WORKSPACES);
    vi.stubGlobal("fetch", vi.fn(() => okResponse()));

    const { result } = renderHook(() => useLogout(), { wrapper: wrapper(qc) });
    await act(async () => {
      await result.current.mutateAsync();
    });

    expect(qc.getQueryData(["my-workspaces"])).toBeUndefined();
    expect(qc.getQueryData(["me"])).toBeNull();
  });

  it("login success leaves no stale my-workspaces for the new session", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["my-workspaces"], STALE_WORKSPACES);
    vi.stubGlobal("fetch", vi.fn(() => okResponse()));

    const { result } = renderHook(() => useLogin(), { wrapper: wrapper(qc) });
    await act(async () => {
      await result.current.mutateAsync({ email: "new@example.com", password: "pw" });
    });

    expect(qc.getQueryData(["my-workspaces"])).toBeUndefined();
  });

  it("awaits the me refetch, so a caller that navigates right after mutateAsync sees fresh data", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let meFetchCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = String(input);
        if (url === "/api/auth/me") {
          meFetchCount += 1;
          return delayedJsonResponse({ user_id: "u2", email: "new@example.com", platform_roles: [], permissions: [] });
        }
        if (url === "/api/auth/login") return okResponse();
        return Promise.reject(new Error(`unexpected fetch: ${url}`));
      }),
    );

    const w = wrapper(qc);
    // Mount an active `["me"]` observer — the same shape `AuthProvider` keeps
    // mounted app-wide — which is what makes `invalidateQueries` below actually
    // trigger a network refetch instead of just marking the query stale.
    renderHook(() => useQuery({ queryKey: ["me"], queryFn: () => fetch("/api/auth/me").then((r) => r.json()) }), {
      wrapper: w,
    });
    // Wait for the *query* to settle, not just for the mock to have been called —
    // react-query dedupes a refetch into an already-in-flight fetch for the same
    // key, so triggering login while the initial fetch is still pending would
    // make the assertions below pass without a second network call at all.
    await waitFor(() => {
      expect(meFetchCount).toBe(1);
      expect(qc.getQueryState(["me"])?.fetchStatus).toBe("idle");
    });

    const { result } = renderHook(() => useLogin(), { wrapper: w });
    await act(async () => {
      await result.current.mutateAsync({ email: "new@example.com", password: "pw" });
    });

    // The refetch must have completed by the time mutateAsync resolves, not
    // merely started — otherwise a caller that navigates right after `await
    // mutateAsync()` (e.g. `LoginPage`) can still render off stale `me`.
    expect(meFetchCount).toBe(2);
    expect(qc.getQueryState(["me"])?.fetchStatus).toBe("idle");
  });

  it("exiting impersonation clears the target's cache before mutateAsync resolves", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["my-workspaces"], STALE_WORKSPACES);
    vi.stubGlobal("fetch", vi.fn(() => okResponse()));

    const { result } = renderHook(() => useExitImpersonation(), { wrapper: wrapper(qc) });
    await act(async () => {
      await result.current.mutateAsync();
    });

    expect(qc.getQueryData(["my-workspaces"])).toBeUndefined();
  });

  it("starting impersonation clears the admin's cache before mutateAsync resolves", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    qc.setQueryData(["my-workspaces"], STALE_WORKSPACES);
    vi.stubGlobal("fetch", vi.fn(() => jsonResponse({})));

    const { result } = renderHook(() => useStartPlatformImpersonation(), { wrapper: wrapper(qc) });
    await act(async () => {
      await result.current.mutateAsync({ target_user_id: "u2", reason: "support" });
    });

    expect(qc.getQueryData(["my-workspaces"])).toBeUndefined();
  });

  // Round 1 review of the rule dialog (spec C8.4): a draft is the signed-in user's, kept in memory;
  // every identity switch clears the user-scoped cache, and the drafts with it.
  it("forgets every rule draft with the user-scoped cache", () => {
    saveRuleDraft("k", { name: "kept", kind: { type: "mask", mask_type: "MASK" }, selector: { type: "tag", key: "pii" } });
    clearUserScopedCache(new QueryClient());
    expect(loadRuleDraft("k")).toBeUndefined();
  });
});
