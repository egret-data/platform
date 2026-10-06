import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// Mock only the shared `apiFetch` seam the generated client goes through.
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const apiFetch = vi.fn(async (_path: string, _init?: RequestInit) => new Response(null, { status: 204 }));
vi.mock("@/api/client", async (importActual) => {
  const actual = await importActual<typeof import("@/api/client")>();
  return {
    ...actual,
    apiFetch: (...a: unknown[]) => apiFetch(...(a as Parameters<typeof apiFetch>)),
  };
});

import { useAppCapabilities, useApps, useAppMutations } from "../apps";
import { getAppsGetOneQueryKey, getAppsListQueryKey } from "../generated/apps/apps";

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  const wrapper = ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return { invalidate, wrapper };
}

beforeEach(() => {
  apiFetch.mockReset();
  apiFetch.mockImplementation(async () => new Response(null, { status: 204 }));
});

describe("useApps", () => {
  it("lists live apps with no query string", async () => {
    // Arrange
    apiFetch.mockImplementation(async () => json(200, [{ id: "a1" }]));
    const { wrapper } = setup();

    // Act
    const { result } = renderHook(() => useApps("w"), { wrapper });

    // Assert
    await waitFor(() => expect(result.current.data).toEqual([{ id: "a1" }]));
    expect(apiFetch.mock.calls[0][0]).toBe("/ws/w/apps");
  });

  it("asks for the trash with trashed=true", async () => {
    // Arrange
    apiFetch.mockImplementation(async () => json(200, []));
    const { wrapper } = setup();

    // Act
    const { result } = renderHook(() => useApps("w", { trashed: true }), { wrapper });

    // Assert
    await waitFor(() => expect(result.current.data).toEqual([]));
    expect(apiFetch.mock.calls[0][0]).toBe("/ws/w/apps?trashed=true");
  });
});

describe("useAppCapabilities", () => {
  it("reads the workspace answer from its own route", async () => {
    // Arrange
    apiFetch.mockImplementation(async () => json(200, { can_create: false }));
    const { wrapper } = setup();

    // Act
    const { result } = renderHook(() => useAppCapabilities("w"), { wrapper });

    // Assert
    await waitFor(() => expect(result.current.data).toEqual({ can_create: false }));
    expect(apiFetch.mock.calls[0][0]).toBe("/ws/w/apps/capabilities");
  });
});

describe("useAppMutations", () => {
  // Every void mutation: the method and path it must hit. The list key is a PREFIX of the
  // trashed list's key (`[url]` vs `[url, { trashed: true }]`), so one invalidation covers both.
  const VOID_CASES = [
    { name: "trash", method: "POST", path: "/ws/w/apps/a1/trash" },
    { name: "restore", method: "POST", path: "/ws/w/apps/a1/restore" },
    { name: "remove", method: "DELETE", path: "/ws/w/apps/a1" },
    { name: "addBookmark", method: "POST", path: "/ws/w/apps/a1/bookmark" },
    { name: "removeBookmark", method: "DELETE", path: "/ws/w/apps/a1/bookmark" },
  ] as const;

  it.each(VOID_CASES)("$name sends $method $path and invalidates the list", async ({ name, method, path }) => {
    // Arrange
    const { invalidate, wrapper } = setup();
    const { result } = renderHook(() => useAppMutations("w"), { wrapper });

    // Act
    await act(async () => { await result.current[name].mutateAsync("a1"); });

    // Assert
    expect(apiFetch).toHaveBeenCalledTimes(1);
    const [gotPath, init] = apiFetch.mock.calls[0] as [string, RequestInit];
    expect(gotPath).toBe(path);
    expect(init.method).toBe(method);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: getAppsListQueryKey("w") });
  });

  it("update PATCHes name and description, and invalidates the list and that app", async () => {
    // Arrange
    apiFetch.mockImplementation(async () => json(200, { id: "a1", name: "New" }));
    const { invalidate, wrapper } = setup();
    const { result } = renderHook(() => useAppMutations("w"), { wrapper });

    // Act
    let got: { id: string } | undefined;
    await act(async () => {
      got = await result.current.update.mutateAsync({ id: "a1", name: "New", description: "d" });
    });

    // Assert
    const [path, init] = apiFetch.mock.calls[0] as [string, RequestInit];
    expect(path).toBe("/ws/w/apps/a1");
    expect(init.method).toBe("PATCH");
    expect(JSON.parse(init.body as string)).toEqual({ name: "New", description: "d" });
    expect(got?.id).toBe("a1");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: getAppsListQueryKey("w") });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: getAppsGetOneQueryKey("w", "a1") });
  });

  it("duplicate POSTs to the duplicate route, returns the copy and invalidates the list", async () => {
    // Arrange
    apiFetch.mockImplementation(async () => json(201, { id: "copy1", name: "Copy of X" }));
    const { invalidate, wrapper } = setup();
    const { result } = renderHook(() => useAppMutations("w"), { wrapper });

    // Act
    let copy: { id: string } | undefined;
    await act(async () => { copy = await result.current.duplicate.mutateAsync("src1"); });

    // Assert
    const [path, init] = apiFetch.mock.calls[0] as [string, RequestInit];
    expect(path).toBe("/ws/w/apps/src1/duplicate");
    expect(init.method).toBe("POST");
    expect(copy?.id).toBe("copy1");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: getAppsListQueryKey("w") });
  });

  it("the list key is a prefix of the trashed list's key, so one invalidation reaches both", () => {
    expect(getAppsListQueryKey("w", { trashed: true }).slice(0, 1)).toEqual([...getAppsListQueryKey("w")]);
  });
});
