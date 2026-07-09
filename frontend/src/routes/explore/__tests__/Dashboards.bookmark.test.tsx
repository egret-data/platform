import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import { createElement, type ReactNode } from "react";

// The dashboards module is migrated to the generated client → mutator → `apiFetch`.
const apiFetch = vi.fn();
const listData = { current: [] as unknown[] };
const jsonRes = (body: unknown) => ({ status: 200, headers: new Headers({ "content-type": "application/json" }), json: () => Promise.resolve(body) });
vi.mock("@/api/client", () => ({
  apiJson: vi.fn(),
  apiFetch: (...a: unknown[]) => apiFetch(...a),
  ApiError: class ApiError extends Error {},
  UnauthorizedError: class UnauthorizedError extends Error {},
}));
vi.mock("react-router-dom", async (orig) => ({ ...(await orig() as object), useParams: () => ({ workspace: "eng" }) }));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

import Dashboards from "../Dashboards";

const wrap = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, createElement(TooltipProvider, null, createElement(MemoryRouter, null, children)));

describe("Dashboards bookmark", () => {
  beforeEach(() => {
    apiFetch.mockReset();
    listData.current = [];
    apiFetch.mockImplementation((url: string) =>
      Promise.resolve(jsonRes(/\/dashboards(\?|$)/.test(url) ? listData.current : null)),
    );
  });
  it("clicking the star bookmarks the dashboard (POST /bookmark)", async () => {
    listData.current = [{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), bookmarked: false }];
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    fireEvent.click(screen.getByRole("button", { name: "bookmark One" }));
    await waitFor(() => expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/d1/bookmark", expect.objectContaining({ method: "POST" })));
  });

  it("clicking a bookmarked star removes it (DELETE /bookmark)", async () => {
    listData.current = [{ id: "d1", name: "One", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), bookmarked: true }];
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("One");
    fireEvent.click(screen.getByRole("button", { name: "unbookmark One" }));
    await waitFor(() => expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/d1/bookmark", expect.objectContaining({ method: "DELETE" })));
  });

  it("the Bookmarks filter hides non-bookmarked dashboards", async () => {
    listData.current = [
      { id: "d1", name: "Starred", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), bookmarked: true },
      { id: "d2", name: "Plain", description: "", created_by: null, card_count: 0, updated_at: new Date().toISOString(), bookmarked: false },
    ];
    render(<Dashboards />, { wrapper: wrap });
    await screen.findByText("Starred");
    expect(screen.getByText("Plain")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("Starred")).toBeTruthy();
    expect(screen.queryByText("Plain")).toBeNull();
  });
});
