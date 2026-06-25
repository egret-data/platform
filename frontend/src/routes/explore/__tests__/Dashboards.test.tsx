import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import Dashboards from "../Dashboards";

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: [{ id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z" }], isLoading: false }),
  useDashboardMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "x" }), isPending: false },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    duplicate: { mutateAsync: vi.fn().mockResolvedValue({ id: "x" }) },
    trash: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    restore: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
        <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Dashboards list", () => {
  it("lists dashboards with their card counts", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText((t) => t.includes("3"))).toBeInTheDocument();
  });
});
