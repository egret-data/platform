import { expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// The shell resolves the workspace NAME for its subtitle through
// `useMyWorkspaces`. Mocked rather than left to fire a real `fetch` in jsdom —
// and mocked with a real row, because "does the subtitle name the workspace"
// is one of the things this file has to answer.
vi.mock("@/api/hooks");

import * as hooks from "@/api/hooks";
import DataApiLayout from "../DataApiLayout";

// The real route is `/explore/:workspace/*` (App.tsx) — `:workspace`, not
// `:ws`. A test tree that invents a different param name cannot see a shell
// reading the wrong one: the lookup silently misses, the subtitle quietly
// drops the workspace name, and the test stays green. So this mirrors App.tsx.
// `endpoints/:id` is likewise mounted as a SIBLING leaf of `endpoints` under
// `data-api`, not nested under it, so a regression in the nav's active-matching
// shows up here the same way it would in the real tree.
function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/explore/:workspace/data-api" element={<DataApiLayout />}>
            <Route path="endpoints" element={<p>endpoints pane</p>} />
            <Route path="endpoints/:id" element={<p>endpoint detail pane</p>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function mockWorkspaces(rows: { slug: string; name: string }[]) {
  vi.mocked(hooks.useMyWorkspaces).mockReturnValue(
    { data: rows, isLoading: false, isError: false } as unknown as ReturnType<typeof hooks.useMyWorkspaces>,
  );
}

it("marks the active section and renders the outlet", () => {
  mockWorkspaces([{ slug: "analytics", name: "Analytics" }]);
  renderAt("/explore/analytics/data-api/endpoints");
  // The shell renders a desktop nav AND a mobile strip; both are in the DOM
  // under JSDOM because there is no CSS, so every link appears twice.
  const links = screen.getAllByRole("link", { name: "Endpoints" });
  expect(links.length).toBeGreaterThan(1);
  expect(links.some((l) => l.getAttribute("aria-current") === "page")).toBe(true);
  expect(screen.getByText("endpoints pane")).toBeInTheDocument();
});

it("keeps Endpoints marked active on an endpoint's detail route", () => {
  mockWorkspaces([{ slug: "analytics", name: "Analytics" }]);
  // endpoints/:id is a sibling leaf, not a child of endpoints — an `end`-matched
  // NavLink would go dark here even though the user is still squarely inside
  // the Endpoints section (this is what EndpointDetail's own nav renders into).
  renderAt("/explore/analytics/data-api/endpoints/e1");
  const links = screen.getAllByRole("link", { name: "Endpoints" });
  expect(links.length).toBeGreaterThan(1);
  expect(links.every((l) => l.getAttribute("aria-current") === "page")).toBe(true);
  expect(screen.getByText("endpoint detail pane")).toBeInTheDocument();
});

// Final-review finding (Important #2): the shell read `:ws`, a param the app's
// route tree does not define, so `slug` was always `undefined`, the workspace
// lookup never matched, and this branch was dead code in production. The
// generic fallback renders either way — the assertion has to be that the NAME
// is there, not merely that a subtitle is.
it("names the workspace in the subtitle, resolved from the :workspace route param", async () => {
  mockWorkspaces([{ slug: "analytics", name: "Analytics" }]);
  renderAt("/explore/analytics/data-api/endpoints");
  expect(
    await screen.findByText("Publish parameterized queries as REST endpoints for Analytics."),
  ).toBeInTheDocument();
});

it("falls back to the generic subtitle when the slug matches no workspace", () => {
  mockWorkspaces([{ slug: "somewhere-else", name: "Elsewhere" }]);
  renderAt("/explore/analytics/data-api/endpoints");
  expect(screen.getByText("Publish parameterized queries as REST endpoints.")).toBeInTheDocument();
});
