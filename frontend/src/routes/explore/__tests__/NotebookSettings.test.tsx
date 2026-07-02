import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────

const settingsMock = vi.fn();
const saveMutateAsync = vi.fn();
const saveMutation = vi.fn();

vi.mock("@/api/notebookSettings", () => ({
  useNotebookSettings: () => settingsMock(),
  useSaveNotebookSettings: () => saveMutation(),
}));

// sonner toast
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import NotebookSettings from "@/routes/explore/NotebookSettings";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/notebook-settings"]}>
        <Routes>
          <Route
            path="/explore/:workspace/notebook-settings"
            element={<NotebookSettings />}
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const mockSettings = {
  engines: [
    { id: "eng-1", name: "prod-trino", engine: "trino", enabled: true },
    { id: "eng-2", name: "analytics-sr", engine: "starrocks", enabled: false },
  ],
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  settingsMock.mockReset();
  saveMutation.mockReset();
  saveMutateAsync.mockReset();

  settingsMock.mockReturnValue({
    data: mockSettings,
    isLoading: false,
    isError: false,
    error: null,
  });

  saveMutation.mockReturnValue({ mutateAsync: saveMutateAsync, isPending: false });
  saveMutateAsync.mockResolvedValue(mockSettings);
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("NotebookSettings page", () => {
  it("renders engine rows with correct names and engine-type badges", () => {
    renderAt();
    expect(screen.getByText("prod-trino")).toBeInTheDocument();
    expect(screen.getByText("analytics-sr")).toBeInTheDocument();
    expect(screen.getByText("trino")).toBeInTheDocument();
    expect(screen.getByText("starrocks")).toBeInTheDocument();
  });

  it("seeds engine checkboxes from enabled field", () => {
    renderAt();
    const enableEngine1 = screen.getByRole("checkbox", {
      name: /enable engine prod-trino/i,
    });
    const enableEngine2 = screen.getByRole("checkbox", {
      name: /enable engine analytics-sr/i,
    });
    expect(enableEngine1).toBeChecked();
    expect(enableEngine2).not.toBeChecked();
  });

  it("toggling a checkbox then saving calls useSaveNotebookSettings with expected ids", async () => {
    const user = userEvent.setup();
    renderAt();

    // Toggle eng-2 on (was false), leave eng-1 on.
    const engine2Checkbox = screen.getByRole("checkbox", {
      name: /enable engine analytics-sr/i,
    });
    await user.click(engine2Checkbox);

    await user.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(saveMutateAsync).toHaveBeenCalledWith({
        engine_ids: expect.arrayContaining(["eng-1", "eng-2"]),
      });
    });
    expect(saveMutateAsync).toHaveBeenCalledTimes(1);
  });

  it("unchecking an enabled item removes it from the saved ids", async () => {
    const user = userEvent.setup();
    renderAt();

    // Uncheck eng-1 (was enabled).
    const engine1Checkbox = screen.getByRole("checkbox", {
      name: /enable engine prod-trino/i,
    });
    await user.click(engine1Checkbox);

    await user.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(saveMutateAsync).toHaveBeenCalledWith({
        engine_ids: [],
      });
    });
  });

  it("shows 403 admin-required message when Save returns a 403 error", async () => {
    const user = userEvent.setup();

    const { ApiError } = await import("@/api/client");
    saveMutateAsync.mockRejectedValue(new ApiError(403, "Forbidden"));

    renderAt();

    await user.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(
        screen.getByText("Requires workspace-admin to change notebook settings."),
      ).toBeInTheDocument();
    });
  });

  it("shows loading skeleton while data is loading", () => {
    settingsMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    // Heading still shows
    expect(screen.getByText("Notebook settings")).toBeInTheDocument();
    // No checkbox rows while loading
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("shows empty state when workspace has no engines", () => {
    settingsMock.mockReturnValue({
      data: { engines: [] },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(
      screen.getByText("No engines configured for this workspace yet."),
    ).toBeInTheDocument();
  });
});
