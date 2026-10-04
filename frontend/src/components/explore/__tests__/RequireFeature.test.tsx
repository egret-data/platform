/**
 * RequireFeature tests
 *
 * RequireFeature reads `useWorkspaceFeatures`, which reads `useMyWorkspaces`
 * (`/me/workspaces`), so every render needs:
 *   - QueryClientProvider
 *   - MemoryRouter under a route exposing the :workspace param
 *   - a mocked `fetch`
 */
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, test, vi } from "vitest";
import { RequireFeature } from "@/components/explore/RequireFeature";

afterEach(() => vi.restoreAllMocks());

function renderGuarded(feature: string, fetchImpl: () => Promise<Response> = () => new Promise(() => {})) {
  vi.spyOn(globalThis, "fetch").mockImplementation(fetchImpl);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/apps"]}>
        <Routes>
          <Route
            path="/explore/:workspace/*"
            element={
              <RequireFeature feature={feature}>
                <span>page content</span>
              </RequireFeature>
            }
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const wsResponse = (features: string[]) =>
  Promise.resolve(
    new Response(JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features }]), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  );

describe("RequireFeature", () => {
  test("blocks with Forbidden once the workspace's features load without it", async () => {
    renderGuarded("apps", () => wsResponse(["sql_lab", "datasets"]));
    await waitFor(() => expect(screen.getByText(/Forbidden/i)).toBeInTheDocument());
    expect(screen.queryByText("page content")).not.toBeInTheDocument();
  });

  test("renders children once the workspace's features load with it", async () => {
    renderGuarded("apps", () => wsResponse(["apps"]));
    await waitFor(() => expect(screen.getByText("page content")).toBeInTheDocument());
    expect(screen.queryByText(/Forbidden/i)).not.toBeInTheDocument();
  });

  test("does not flash Forbidden while /me/workspaces is still loading", () => {
    // fetchImpl never resolves — assertions run against the pending state.
    renderGuarded("apps");
    expect(screen.getByText("page content")).toBeInTheDocument();
    expect(screen.queryByText(/Forbidden/i)).not.toBeInTheDocument();
  });
});
