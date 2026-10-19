import { render } from "@testing-library/react";
import { vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import OverviewPage from "@/routes/governance/overview/OverviewPage";

export type Answer = { status: number; body: unknown } | "never";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/**
 * Stubs `fetch` for the Overview's two reads, through the real generated
 * client: `GET …/governance/attention` and `GET …/governance/changes`
 * (answered per call from its search params). Returns the changes calls.
 */
export function serve(attention: () => Answer, changes: (q: URLSearchParams) => Answer = () => "never") {
  const changesCalls: URLSearchParams[] = [];
  let attentionCalls = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const reply = (a: Answer) =>
        a === "never" ? new Promise<Response>(() => {}) : Promise.resolve(json(a.status, a.body));
      if (url.pathname === "/api/control-plane/governance/attention") {
        attentionCalls += 1;
        return reply(attention());
      }
      if (url.pathname === "/api/control-plane/governance/changes") {
        changesCalls.push(url.searchParams);
        return reply(changes(url.searchParams));
      }
      return Promise.resolve(json(404, { error: "not found" }));
    }),
  );
  return { changesCalls, attentionCalls: () => attentionCalls };
}

export const ADMIN: Me = { user_id: "a1", email: "a@example.com", platform_roles: ["platform_admin"], permissions: [] };

export function openOverview(me: Me = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/governance"]}>
            <Routes>
              <Route path="/governance" element={<OverviewPage />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}
