import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────
const storageConnsMock = vi.fn();
const createMutateAsync = vi.fn();
const createMutation = vi.fn();
const updateMutateAsync = vi.fn();
const updateMutation = vi.fn();
const deleteMutateAsync = vi.fn();
const deleteMutation = vi.fn();

vi.mock("@/api/storage", () => ({
  useStorageConns: () => storageConnsMock(),
  useCreateStorageConn: () => createMutation(),
  useUpdateStorageConn: () => updateMutation(),
  useDeleteStorageConn: () => deleteMutation(),
  // FileBrowser (rendered on Browse) consumes these.
  useStorageObjects: () => ({
    data: { prefix: "", folders: [], objects: [] },
    isLoading: false,
    isError: false,
    error: null,
  }),
  useUploadObject: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteObject: () => ({ mutate: vi.fn(), isPending: false }),
  useCreateFolder: () => ({ mutate: vi.fn(), isPending: false }),
  useMoveObject: () => ({ mutate: vi.fn(), isPending: false }),
  downloadObject: vi.fn(),
}));

import StoragePage from "@/routes/explore/StoragePage";

// ── Render helper ─────────────────────────────────────────────────────────────
function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/storage"]}>
        <Routes>
          <Route path="/explore/:workspace/storage" element={<StoragePage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────
const mockConn1 = {
  id: "conn-1",
  name: "prod-lakehouse",
  purpose: "catalog",
  provider: "s3",
  endpoint: "https://minio.corp.local",
  region: "us-east-1",
  bucket: "my-data-lake",
  key_prefix: "warehouse/eng",
  access_key: "AKIAIOSFODNN7EXAMPLE",
  path_style: true,
  created_at: "2025-01-01T00:00:00Z",
  has_secret: true,
};

const mockConn2 = {
  id: "conn-2",
  name: "backup-store",
  purpose: null,
  provider: "s3",
  endpoint: null,
  region: "eu-west-1",
  bucket: "backup-bucket",
  key_prefix: null,
  access_key: "AKIAIOSFODNN7BACKUP",
  path_style: false,
  created_at: "2025-02-01T00:00:00Z",
  has_secret: false,
};

// ── Setup ─────────────────────────────────────────────────────────────────────
beforeEach(() => {
  storageConnsMock.mockReset();
  createMutation.mockReset();
  updateMutation.mockReset();
  deleteMutation.mockReset();
  createMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  deleteMutateAsync.mockReset();

  // Default: successful load of two connections
  storageConnsMock.mockReturnValue({
    data: [mockConn1, mockConn2],
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

  deleteMutation.mockReturnValue({
    mutate: deleteMutateAsync,
    isPending: false,
  });

  createMutateAsync.mockResolvedValue(mockConn1);
  updateMutateAsync.mockResolvedValue(mockConn1);
  deleteMutateAsync.mockResolvedValue(undefined);
});

describe("StoragePage", () => {
  // ── List rendering ────────────────────────────────────────────────────────

  it("renders the connections list with name, bucket, provider, and secret badge", () => {
    renderAt();

    // conn1
    expect(screen.getByText("prod-lakehouse")).toBeInTheDocument();
    expect(screen.getByText("my-data-lake")).toBeInTheDocument();
    // provider badge (may be multiple "s3" badges)
    expect(screen.getAllByText("s3").length).toBeGreaterThanOrEqual(1);
    // has_secret=true → "Configured"
    expect(screen.getByText("Configured")).toBeInTheDocument();

    // conn2
    expect(screen.getByText("backup-store")).toBeInTheDocument();
    expect(screen.getByText("backup-bucket")).toBeInTheDocument();
    // has_secret=false → "Not set"
    expect(screen.getByText("Not set")).toBeInTheDocument();

    // Secret strings must never appear
    expect(screen.queryByText(/SUPER_SECRET/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/secret_key/i)).not.toBeInTheDocument();
  });

  it("shows 'AWS default' for connections with no endpoint", () => {
    renderAt();
    expect(screen.getByText("AWS default")).toBeInTheDocument();
  });

  it("shows empty state when no connections exist", () => {
    storageConnsMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText("No storage connections yet.")).toBeInTheDocument();
  });

  it("shows skeleton while loading", () => {
    storageConnsMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    // Skeleton renders — no table rows
    expect(screen.queryByText("prod-lakehouse")).not.toBeInTheDocument();
  });

  // ── 403 ───────────────────────────────────────────────────────────────────

  it("shows workspace-admin notice on 403", () => {
    storageConnsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();

    expect(
      screen.getByText("Requires workspace-admin to manage storage."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add connection")).not.toBeInTheDocument();
  });

  // ── Generic error ─────────────────────────────────────────────────────────

  it("shows generic error alert on unexpected error status", () => {
    storageConnsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 500 },
    });
    renderAt();

    expect(screen.getByText("Failed to load storage connections")).toBeInTheDocument();
  });

  // ── Add connection dialog — Provider select ───────────────────────────────

  it("Add dialog renders a Provider select showing 'S3 / MinIO' as the selected option", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add storage connection"));

    // Radix renders the selected value in a <span data-slot="select-value">
    // and also in a hidden <option>; getAllByText handles both occurrences.
    const instances = screen.getAllByText("S3 / MinIO");
    expect(instances.length).toBeGreaterThanOrEqual(1);
  });

  it("GCS and Azure provider options carry the disabled attribute", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add storage connection"));

    // Radix renders a native <select> (aria-hidden) alongside its custom UI.
    // Check the native <option> elements for the disabled attribute — reliable in jsdom.
    const nativeOptions = document.querySelectorAll<HTMLOptionElement>("option");
    const gcsNative = Array.from(nativeOptions).find((o) => o.value === "gcs");
    const azureNative = Array.from(nativeOptions).find((o) => o.value === "azure");

    expect(gcsNative).toBeTruthy();
    expect(azureNative).toBeTruthy();
    expect(gcsNative?.disabled).toBe(true);
    expect(azureNative?.disabled).toBe(true);
  });

  it("submitting the create form includes provider: 's3' in the payload", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add storage connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), { target: { value: "provider-test" } });
    fireEvent.change(screen.getByLabelText(/^region/i), { target: { value: "us-east-1" } });
    fireEvent.change(screen.getByLabelText(/^bucket/i), { target: { value: "test-bucket" } });
    fireEvent.change(screen.getByLabelText(/^access key/i), { target: { value: "AKIATEST" } });
    fireEvent.change(screen.getByLabelText(/^secret key/i), { target: { value: "secretTest" } });

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ provider: "s3" }),
      );
    });
  });

  // ── Add connection dialog ─────────────────────────────────────────────────

  it("opens the Add dialog when 'Add connection' is clicked", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));
    await waitFor(() => {
      expect(screen.getByText("Add storage connection")).toBeInTheDocument();
    });
  });

  it("calls createMutation with correct body when Add form is submitted", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add storage connection"));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "new-conn" },
    });
    fireEvent.change(screen.getByLabelText(/^region/i), {
      target: { value: "ap-southeast-1" },
    });
    fireEvent.change(screen.getByLabelText(/^bucket/i), {
      target: { value: "new-bucket" },
    });
    fireEvent.change(screen.getByLabelText(/^access key/i), {
      target: { value: "AKIANEWKEY" },
    });
    fireEvent.change(screen.getByLabelText(/^secret key/i), {
      target: { value: "newSecret123" },
    });

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          name: "new-conn",
          region: "ap-southeast-1",
          bucket: "new-bucket",
          access_key: "AKIANEWKEY",
          secret_key: "newSecret123",
          provider: "s3",
        }),
      );
    });
  });

  it("shows the server error message in the dialog on create failure", async () => {
    createMutateAsync.mockRejectedValue({
      message: "Bucket unreachable: connection refused",
    });

    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add connection/i }));

    await waitFor(() => screen.getByText("Add storage connection"));

    // Fill required fields
    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "bad-conn" },
    });
    fireEvent.change(screen.getByLabelText(/^region/i), {
      target: { value: "us-east-1" },
    });
    fireEvent.change(screen.getByLabelText(/^bucket/i), {
      target: { value: "bad-bucket" },
    });
    fireEvent.change(screen.getByLabelText(/^access key/i), {
      target: { value: "AKIAKEY" },
    });
    fireEvent.change(screen.getByLabelText(/^secret key/i), {
      target: { value: "secret" },
    });

    fireEvent.click(screen.getByRole("button", { name: /add connection/i, hidden: false }));

    await waitFor(() => {
      expect(
        screen.getByText("Bucket unreachable: connection refused"),
      ).toBeInTheDocument();
    });
  });

  // ── Edit dialog ───────────────────────────────────────────────────────────

  it("opens the Edit dialog with the connection's name prefilled", async () => {
    renderAt();

    // Click the first Edit button (for conn1)
    const editButtons = screen.getAllByRole("button", { name: /edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => {
      expect(screen.getByText("Edit storage connection")).toBeInTheDocument();
    });

    // Name is prefilled
    const nameInput = screen.getByDisplayValue("prod-lakehouse");
    expect(nameInput).toBeInTheDocument();
  });

  it("calls updateMutation with correct body when Edit form is submitted", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit storage connection"));

    // Change bucket
    const bucketInput = screen.getByDisplayValue("my-data-lake");
    fireEvent.change(bucketInput, { target: { value: "updated-bucket" } });

    // Leave secret blank → should not send it (null)
    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "conn-1",
          input: expect.objectContaining({
            bucket: "updated-bucket",
            secret_key: null,
          }),
        }),
      );
    });
  });

  it("sends secret_key when user types a new secret in the Edit dialog", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit storage connection"));

    const secretInput = screen.getByLabelText(/^secret key/i);
    fireEvent.change(secretInput, { target: { value: "newSecret!" } });

    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          input: expect.objectContaining({ secret_key: "newSecret!" }),
        }),
      );
    });
  });

  // ── Browse ────────────────────────────────────────────────────────────────

  it("Browse button is enabled and opens the file browser", async () => {
    renderAt();

    const browseBtn = screen.getByRole("button", {
      name: /browse files for prod-lakehouse/i,
    });
    expect(browseBtn).not.toBeDisabled();

    fireEvent.click(browseBtn);

    // FileBrowser detail view shows the connection name + a Back action.
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /back to connections/i }),
      ).toBeInTheDocument();
    });
    // The connections list table is no longer shown.
    expect(screen.queryByText("backup-store")).not.toBeInTheDocument();
  });

  // ── Delete ────────────────────────────────────────────────────────────────

  it("calls deleteMutation when delete is confirmed", async () => {
    renderAt();

    // Click the first "Delete" action button in the table (aria-label "Delete prod-lakehouse")
    const deleteButton = screen.getByRole("button", {
      name: /delete prod-lakehouse/i,
    });
    fireEvent.click(deleteButton);

    // ConfirmDeleteDialog opens
    await waitFor(() => {
      expect(screen.getByText(/Delete «prod-lakehouse»/)).toBeInTheDocument();
    });

    // Type "delete" into the placeholder="delete" input
    const confirmInput = screen.getByPlaceholderText("delete");
    fireEvent.change(confirmInput, { target: { value: "delete" } });

    // The destructive "Delete" button inside the dialog becomes enabled
    const confirmBtn = screen.getByRole("button", { name: /^delete$/i });
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith("conn-1");
    });
  });
});
