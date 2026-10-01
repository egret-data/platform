import type { ReactNode } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, test, vi } from "vitest";
import { useDefaultExplorePath, useWorkspaceFeatures } from "@/hooks/useWorkspaceFeatures";

afterEach(() => vi.restoreAllMocks());

const wsResponse = (features: string[]) =>
  Promise.resolve(
    new Response(JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features }]), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  );

function wrap(fetchImpl: () => Promise<Response> = () => new Promise(() => {})) {
  vi.spyOn(globalThis, "fetch").mockImplementation(fetchImpl);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/sql-lab"]}>
        <Routes>
          <Route path="/explore/:workspace/*" element={children} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe("useWorkspaceFeatures", () => {
  test("returns every key while /me/workspaces is still loading", () => {
    const { result } = renderHook(() => useWorkspaceFeatures(), { wrapper: wrap() });
    expect(result.current).toEqual([
      "sql_lab",
      "datasets",
      "charts",
      "dashboards",
      "notebooks",
      "alerts",
      "data_api",
      "workflows",
      "apps",
    ]);
  });

  test("returns the current workspace's actual features once loaded", async () => {
    const { result } = renderHook(() => useWorkspaceFeatures(), {
      wrapper: wrap(() => wsResponse(["sql_lab", "charts"])),
    });
    await waitFor(() => expect(result.current).toEqual(["sql_lab", "charts"]));
  });
});

describe("useDefaultExplorePath", () => {
  test("while loading, defaults to sql-lab (first in nav order)", () => {
    const { result } = renderHook(() => useDefaultExplorePath(), { wrapper: wrap() });
    expect(result.current).toBe("sql-lab");
  });

  test("picks the first enabled module in nav order when sql_lab is off", async () => {
    const { result } = renderHook(() => useDefaultExplorePath(), {
      wrapper: wrap(() => wsResponse(["charts", "notebooks"])),
    });
    await waitFor(() => expect(result.current).toBe("charts"));
  });

  test("lands on alerts when it is the only enabled module", async () => {
    // Alerts sits LAST in nav order, so it is the module a workspace falls back
    // to rather than one that can ever displace SQL Lab as the default landing
    // page — the ordering consequence of adding an entry to EXPLORE_MODULES.
    const { result } = renderHook(() => useDefaultExplorePath(), {
      wrapper: wrap(() => wsResponse(["alerts"])),
    });
    await waitFor(() => expect(result.current).toBe("alerts"));
  });

  test("falls back to settings/members when every module is disabled", async () => {
    const { result } = renderHook(() => useDefaultExplorePath(), {
      wrapper: wrap(() => wsResponse([])),
    });
    await waitFor(() => expect(result.current).toBe("settings/members"));
  });
});
