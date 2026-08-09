import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import Dashboards from "../Dashboards";

const createMock = vi.fn().mockResolvedValue({ id: "x" });

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: [], isLoading: false }),
  useDashboardMutations: () => ({
    create: { mutateAsync: createMock, isPending: false },
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

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
          <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

function openDialog() {
  renderAt();
  fireEvent.click(screen.getByRole("button", { name: "+ Dashboard" }));
}

describe("New dashboard dialog", () => {
  beforeEach(() => createMock.mockClear());

  it("sends the trimmed description alongside the name", async () => {
    openDialog();
    fireEvent.change(screen.getByPlaceholderText("Name"), { target: { value: "  Ops  " } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "  weekly ops review  " } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createMock).toHaveBeenCalled());
    expect(createMock).toHaveBeenCalledWith({ name: "Ops", description: "weekly ops review" });
  });

  it("omits the description key entirely when the field is left blank", async () => {
    // Not `description: ""`. The column is nullable and the info sidebar keys its placeholder on
    // an absent/empty description; sending a blank string would persist an empty description block
    // rather than leaving the field unset. Asserting on the KEY, not on a falsy value, is the
    // point — `toEqual({name})` would pass for `{name, description: ""}` under a loose matcher.
    openDialog();
    fireEvent.change(screen.getByPlaceholderText("Name"), { target: { value: "Ops" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createMock).toHaveBeenCalled());
    const body = createMock.mock.calls[0][0];
    expect(Object.keys(body)).toEqual(["name"]);
    expect("description" in body).toBe(false);
  });

  it("treats a whitespace-only description as blank", async () => {
    openDialog();
    fireEvent.change(screen.getByPlaceholderText("Name"), { target: { value: "Ops" } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createMock).toHaveBeenCalled());
    expect("description" in createMock.mock.calls[0][0]).toBe(false);
  });
});
