import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import Dashboards from "../Dashboards";

const base = { id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z" };
let row: Record<string, unknown> = { ...base };

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: [row], isLoading: false }),
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
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

const renderList = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <TooltipProvider>
      <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
        <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
      </MemoryRouter>
    </TooltipProvider>
  </QueryClientProvider>,
);

/**
 * A reader could select twenty dashboards and press "Move to Trash" before the server refused.
 * These are DESTRUCTIVE affordances, so the gate matters more here than on the detail page.
 */
describe("the list's mutating affordances", () => {
  it("are withheld when the server says the caller cannot manage", async () => {
    row = { ...base, can_manage: false };
    renderList();
    expect(screen.queryByRole("button", { name: "+ Dashboard" })).toBeNull();
    expect(screen.queryByRole("button", { name: "New folder" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Bulk select" })).toBeNull();

    // The row menu keeps Open — a reader loses the refusals, not the way in.
    await userEvent.click(screen.getByRole("button", { name: /actions/i }));
    expect(screen.getByRole("menuitem", { name: "Open" })).toBeTruthy();
    for (const gone of ["Duplicate", "Move to Trash", "Share…"]) {
      expect(screen.queryByRole("menuitem", { name: gone })).toBeNull();
    }
  });

  it("are offered when the server says they can", async () => {
    row = { ...base, can_manage: true };
    renderList();
    expect(screen.getByRole("button", { name: "+ Dashboard" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "New folder" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Bulk select" })).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: /actions/i }));
    expect(screen.getByRole("menuitem", { name: "Duplicate" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "Move to Trash" })).toBeTruthy();
  });

  it("are offered when the field is absent, so an older response locks nobody out", () => {
    row = { ...base };
    renderList();
    expect(screen.getByRole("button", { name: "+ Dashboard" })).toBeTruthy();
  });
});

describe("the certified badge", () => {
  it("marks a certified dashboard", () => {
    row = { ...base, certified: true };
    renderList();
    expect(screen.getByLabelText("Certified")).toBeTruthy();
  });

  it("is absent on an uncertified one — the mark must mean something", () => {
    row = { ...base, certified: false };
    renderList();
    expect(screen.queryByLabelText("Certified")).toBeNull();
  });
});
