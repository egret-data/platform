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

// SecretPicker pulls the named-secrets list; stub it so the Select renders.
vi.mock("@/api/secrets", () => ({
  useSecrets: () => ({
    data: [{ id: "sec-1", name: "existing-secret", used_by: 0, created_at: "2025-01-01T00:00:00Z" }],
    isLoading: false,
    isError: false,
    error: null,
  }),
}));

import DatabasesPage from "@/routes/explore/DatabasesPage";

// Drive the SecretPicker: pick "New secret" via the hidden native <select>,
// then fill the revealed name + value inputs.
async function fillNewSecret(name: string, value: string) {
  const native = Array.from(
    document.querySelectorAll<HTMLSelectElement>("select"),
  ).find((s) => Array.from(s.options).some((o) => o.value === "__new__"));
  if (!native) throw new Error("SecretPicker native select not found");
  fireEvent.change(native, { target: { value: "__new__" } });
  await waitFor(() => screen.getByLabelText(/new secret name/i));
  fireEvent.change(screen.getByLabelText(/new secret name/i), {
    target: { value: name },
  });
  fireEvent.change(screen.getByLabelText(/new secret value/i), {
    target: { value },
  });
}

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
    expect(screen.getByText("No engines connected yet.")).toBeInTheDocument();
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
      screen.getByText("Requires workspace-admin to manage engines."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add connection")).not.toBeInTheDocument();
  });

  // ── Add dialog ────────────────────────────────────────────────────────────

  it("calls createMutation with engine/host/port/options/secret payload on Add submit", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "new-db" } });
    fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: "db.example.com" } });
    fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: "8080" } });
    fireEvent.change(screen.getByLabelText(/^user$/i), { target: { value: "alice" } });
    fireEvent.change(screen.getByLabelText(/^catalog$/i), { target: { value: "iceberg" } });
    fireEvent.change(screen.getByLabelText(/^schema$/i), { target: { value: "public" } });
    await fillNewSecret("db-secret", "pw123");

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "new-db",
          engine: "trino",
          host: "db.example.com",
          port: 8080,
          new_secret: { name: "db-secret", value: "pw123" },
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

    await waitFor(() => screen.getByText("Edit engine connection"));

    expect(screen.getByDisplayValue("prod-trino")).toBeInTheDocument();
    expect(screen.getByDisplayValue("trino.corp.local")).toBeInTheDocument();
    expect(screen.getByDisplayValue("svc")).toBeInTheDocument();
    expect(screen.getByDisplayValue("iceberg")).toBeInTheDocument();
    expect(screen.getByDisplayValue("analytics")).toBeInTheDocument();
    // On edit the SecretPicker defaults to "Keep current secret" — no plaintext
    // secret input is rendered until the user opts to create a new one.
    expect(screen.queryByLabelText(/new secret value/i)).not.toBeInTheDocument();
  });

  it("omits secret fields on edit when the current secret is kept", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit engine connection"));

    const hostInput = screen.getByDisplayValue("trino.corp.local");
    fireEvent.change(hostInput, { target: { value: "new-host" } });

    // Keep current secret (default on edit) → no secret fields in payload.
    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const call = updateMutateAsync.mock.calls[0][0];
      expect(call.id).toBe("db-1");
      expect(call.input.host).toBe("new-host");
      expect(call.input.secret_id).toBeUndefined();
      expect(call.input.new_secret).toBeUndefined();
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

  // ── Impersonate toggle (Trino only) ───────────────────────────────────────

  it("shows the impersonate toggle when engine is trino in the Add dialog", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    // Default engine is trino — toggle should be present
    expect(
      screen.getByRole("switch", { name: /run queries as the logged-in user/i }),
    ).toBeInTheDocument();
  });

  it("hides the impersonate toggle when engine is starrocks in the Add dialog", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    // Switch to starrocks
    const nativeSelect = Array.from(
      document.querySelectorAll<HTMLSelectElement>("select"),
    ).find((s) => Array.from(s.options).some((o) => o.value === "starrocks"));
    if (nativeSelect) {
      fireEvent.change(nativeSelect, { target: { value: "starrocks" } });
    }

    await waitFor(() => {
      expect(
        screen.queryByRole("switch", { name: /run queries as the logged-in user/i }),
      ).not.toBeInTheDocument();
    });
  });

  it("toggling the impersonate switch and submitting includes options.impersonate: true in the create payload", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "trino-imp" } });
    fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: "trino.example.com" } });
    fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: "8080" } });

    // Toggle the impersonate switch on
    const impersonateSwitch = screen.getByRole("switch", {
      name: /run queries as the logged-in user/i,
    });
    fireEvent.click(impersonateSwitch);

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "trino-imp",
          engine: "trino",
          options: expect.objectContaining({
            impersonate: true,
          }),
        }),
      );
    });
  });

  it("toggling the impersonate switch and submitting includes options.impersonate: true in the update payload", async () => {
    // mockDb1 is trino with no impersonate set
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit engine connection"));

    // Toggle impersonate switch on
    const impersonateSwitch = screen.getByRole("switch", {
      name: /run queries as the logged-in user/i,
    });
    fireEvent.click(impersonateSwitch);

    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const call = updateMutateAsync.mock.calls[0][0];
      expect(call.input.options).toEqual(
        expect.objectContaining({ impersonate: true }),
      );
    });
  });

  // ── Per-user delegation toggle ────────────────────────────────────────────

  it("renders the Per-user delegation switch in the Add dialog", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    expect(
      screen.getByRole("switch", { name: /per-user delegation/i }),
    ).toBeInTheDocument();
  });

  it("toggling Per-user delegation and submitting includes delegated: true in the create payload", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "trino-deleg" } });
    fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: "trino.example.com" } });
    fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: "8080" } });

    fireEvent.click(screen.getByRole("switch", { name: /per-user delegation/i }));

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "trino-deleg",
          delegated: true,
        }),
      );
    });
  });

  it("toggling Per-user delegation and submitting includes delegated: true in the update payload", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit engine connection"));

    fireEvent.click(screen.getByRole("switch", { name: /per-user delegation/i }));

    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const call = updateMutateAsync.mock.calls[0][0];
      expect(call.input.delegated).toBe(true);
    });
  });

  it("defaults delegated to false when the switch is untouched on create", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add engine connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "no-deleg" } });
    fireEvent.change(screen.getByLabelText(/^host$/i), { target: { value: "db.example.com" } });
    fireEvent.change(screen.getByLabelText(/^port$/i), { target: { value: "8080" } });

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ delegated: false }),
      );
    });
  });
});
