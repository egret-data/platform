import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const base = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [], width: "full",
};
let detail: Record<string, unknown> = { ...base };
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return {
    ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({
      update: { mutateAsync: vi.fn(), isPending: false },
      revert: { mutateAsync: vi.fn(), isPending: false },
      setCertification: { mutateAsync: vi.fn(), isPending: false },
    }),
  };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

/**
 * A ws_viewer used to be handed the full editor and only learned the save was refused after
 * making the changes — the server returned 403 on Save, correctly, but the affordance had
 * already promised otherwise.
 *
 * `can_manage` is computed SERVER-side and cannot be derived here: the session's permission list
 * is built from platform grants only (`session.rs` `WHERE ag.scope_type = 'platform'`), so
 * `dashboard:manage`, being workspace-scoped, never appears in it.
 */
describe("the Edit affordance", () => {
  it("is withheld from someone the server says cannot manage", () => {
    detail = { ...base, can_manage: false };
    renderView();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  });

  it("is offered when the server says they can", () => {
    detail = { ...base, can_manage: true };
    renderView();
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
  });

  it("is offered when the field is absent, so an older response cannot lock anyone out", () => {
    // Deliberate: `!== false` rather than `=== true`. A cached or older payload without the
    // field must not remove the editor from a user who has always had it.
    detail = { ...base };
    renderView();
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
  });
});
