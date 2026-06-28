import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────
const databasesMock = vi.fn();
const createMutateAsync = vi.fn();
const createMutation = vi.fn();
const updateMutateAsync = vi.fn();
const updateMutation = vi.fn();
const deleteMutate = vi.fn();
const deleteMutation = vi.fn();
const testMutateAsync = vi.fn();
const testMutation = vi.fn();

vi.mock("@/api/databases", () => ({
  useDatabases: () => databasesMock(),
  useCreateDatabase: () => createMutation(),
  useUpdateDatabase: () => updateMutation(),
  useDeleteDatabase: () => deleteMutation(),
  useTestDatabase: () => testMutation(),
}));

import DatabasesPage from "@/routes/explore/DatabasesPage";

// ── Render helper ─────────────────────────────────────────────────────────────
function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/databases"]}>
        <Routes>
          <Route path="/explore/:workspace/databases" element={<DatabasesPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────
const mockDb1 = {
  id: "db-1",
  name: "prod-trino",
  engine: "trino",
  host: "trino.corp.local",
  port: 8080,
  options: { user: "svc", catalog: "iceberg", database: "analytics", http_scheme: "https" },
  has_secret: true,
  status: "ok",
};

const mockDb2 = {
  id: "db-2",
  name: "warehouse-sr",
  engine: "starrocks",
  host: "starrocks.corp.local",
  port: 9030,
  options: {},
  has_secret: false,
  status: "error",
};

// ── Setup ─────────────────────────────────────────────────────────────────────
beforeEach(() => {
  databasesMock.mockReset();
  createMutation.mockReset();
  updateMutation.mockReset();
  deleteMutation.mockReset();
  testMutation.mockReset();
  createMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  deleteMutate.mockReset();
  testMutateAsync.mockReset();

  databasesMock.mockReturnValue({
    data: [mockDb1, mockDb2],
    isLoading: false,
    isError: false,
    error: null,
  });

  createMutation.mockReturnValue({
    mutateAsync: createMutateAsync,
    isPending: false,
    reset: vi.fn(),
  });
  updateMutation.mockReturnValue({
    mutateAsync: updateMutateAsync,
    isPending: false,
    reset: vi.fn(),
  });
  deleteMutation.mockReturnValue({ mutate: deleteMutate, isPending: false });
  testMutation.mockReturnValue({ mutateAsync: testMutateAsync, isPending: false });

  createMutateAsync.mockResolvedValue(mockDb1);
  updateMutateAsync.mockResolvedValue(mockDb1);
  deleteMutate.mockResolvedValue(undefined);
  testMutateAsync.mockResolvedValue({ ok: true });
});

describe("DatabasesPage", () => {
  // ── List rendering ────────────────────────────────────────────────────────

  it("renders databases with name, engine, host:port, status, and secret badges", () => {
    renderAt();

    expect(screen.getByText("prod-trino")).toBeInTheDocument();
    expect(screen.getByText("trino.corp.local:8080")).toBeInTheDocument();
    // engine badges
    expect(screen.getAllByText("trino").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("starrocks")).toBeInTheDocument();
    // status badges
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(screen.getByText("error")).toBeInTheDocument();
    // secret badges
    expect(screen.getByText("Configured")).toBeInTheDocument();
    expect(screen.getByText("Not set")).toBeInTheDocument();

    expect(screen.getByText("warehouse-sr")).toBeInTheDocument();
    expect(screen.getByText("starrocks.corp.local:9030")).toBeInTheDocument();
  });

  it("does not render the secret as plaintext anywhere", () => {
    renderAt();
    expect(screen.queryByText(/SUPER_SECRET/i)).not.toBeInTheDocument();
    // The DTO carries no plaintext secret; only the has_secret badge is shown.
    expect(screen.queryByDisplayValue(/SUPER_SECRET/i)).not.toBeInTheDocument();
  });

  it("shows empty state when no databases exist", () => {
    databasesMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText("No database connections yet.")).toBeInTheDocument();
  });

  it("shows skeleton while loading", () => {
    databasesMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.queryByText("prod-trino")).not.toBeInTheDocument();
  });

  // ── 403 ───────────────────────────────────────────────────────────────────

  it("shows workspace-admin notice on 403", () => {
    databasesMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();

    expect(
      screen.getByText("Requires workspace-admin to manage databases."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add connection")).not.toBeInTheDocument();
  });

  // ── Add dialog ────────────────────────────────────────────────────────────

  it("calls createMutation with engine/host/port/options/secret payload on Add submit", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add database connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "new-db" } });
    fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: "db.example.com" } });
    fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: "8080" } });
    fireEvent.change(screen.getByLabelText(/^user$/i), { target: { value: "alice" } });
    fireEvent.change(screen.getByLabelText(/^catalog$/i), { target: { value: "iceberg" } });
    fireEvent.change(screen.getByLabelText(/^schema$/i), { target: { value: "public" } });
    fireEvent.change(screen.getByLabelText(/^password$/i), { target: { value: "pw123" } });

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "new-db",
          engine: "trino",
          host: "db.example.com",
          port: 8080,
          secret: "pw123",
          options: expect.objectContaining({
            user: "alice",
            catalog: "iceberg",
            database: "public",
          }),
        }),
      );
    });
  });

  // ── Edit dialog ───────────────────────────────────────────────────────────

  it("opens the Edit dialog with fields (incl options) prefilled", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit database connection"));

    expect(screen.getByDisplayValue("prod-trino")).toBeInTheDocument();
    expect(screen.getByDisplayValue("trino.corp.local")).toBeInTheDocument();
    expect(screen.getByDisplayValue("svc")).toBeInTheDocument();
    expect(screen.getByDisplayValue("iceberg")).toBeInTheDocument();
    expect(screen.getByDisplayValue("analytics")).toBeInTheDocument();
    // Secret is blank on edit
    expect((screen.getByLabelText(/^password$/i) as HTMLInputElement).value).toBe("");
  });

  it("calls updateMutation with null secret when password left blank", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit database connection"));

    const hostInput = screen.getByDisplayValue("trino.corp.local");
    fireEvent.change(hostInput, { target: { value: "new-host" } });

    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "db-1",
          input: expect.objectContaining({ host: "new-host", secret: null }),
        }),
      );
    });
  });

  // ── Test connection ───────────────────────────────────────────────────────

  it("runs Test connection and shows the success outcome", async () => {
    renderAt();

    fireEvent.click(
      screen.getByRole("button", { name: /test connection for prod-trino/i }),
    );

    await waitFor(() => {
      expect(testMutateAsync).toHaveBeenCalledWith("db-1");
    });
    await waitFor(() => {
      expect(screen.getByText("OK")).toBeInTheDocument();
    });
  });

  it("shows the failure message when Test connection fails", async () => {
    testMutateAsync.mockResolvedValue({ ok: false, message: "connection refused" });
    renderAt();

    fireEvent.click(
      screen.getByRole("button", { name: /test connection for prod-trino/i }),
    );

    await waitFor(() => {
      expect(screen.getByText("connection refused")).toBeInTheDocument();
    });
  });

  // ── Delete ────────────────────────────────────────────────────────────────

  it("calls deleteMutation when delete is confirmed", async () => {
    renderAt();

    fireEvent.click(screen.getByRole("button", { name: /delete prod-trino/i }));

    await waitFor(() => {
      expect(screen.getByText(/Delete «prod-trino»/)).toBeInTheDocument();
    });

    const confirmInput = screen.getByPlaceholderText("delete");
    fireEvent.change(confirmInput, { target: { value: "delete" } });

    const confirmBtn = screen.getByRole("button", { name: /^delete$/i });
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(deleteMutate).toHaveBeenCalledWith("db-1");
    });
  });
});
