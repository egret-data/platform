import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const storageMock = vi.fn();
const mutateMock = vi.fn();
const updateStorageMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWorkspaceStorage: () => storageMock(),
  useUpdateStorage: () => updateStorageMock(),
}));

import StoragePage from "@/routes/explore/StoragePage";

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

const mockStorageInfo = {
  provider: "s3",
  endpoint: "https://s3.example.com",
  region: "us-east-1",
  bucket: "my-data-bucket",
  key_prefix: "warehouse/eng",
  access_key: "AKIAIOSFODNN7EXAMPLE",
  has_secret: true,
};

beforeEach(() => {
  storageMock.mockReset();
  updateStorageMock.mockReset();
  mutateMock.mockReset();

  // Default: successful load of custom storage
  storageMock.mockReturnValue({
    data: mockStorageInfo,
    isLoading: false,
    isError: false,
    error: null,
  });

  // Default: idle mutation
  updateStorageMock.mockReturnValue({
    mutateAsync: mutateMock,
    isPending: false,
    reset: vi.fn(),
  });

  mutateMock.mockResolvedValue(mockStorageInfo);
});

describe("StoragePage", () => {
  // ── 200 (custom storage row) ──────────────────────────────────────────────

  it("renders editable form prefilled from server data", () => {
    renderAt();

    // Editable fields should be prefilled
    expect(screen.getByDisplayValue("https://s3.example.com")).toBeInTheDocument();
    expect(screen.getByDisplayValue("us-east-1")).toBeInTheDocument();
    expect(screen.getByDisplayValue("AKIAIOSFODNN7EXAMPLE")).toBeInTheDocument();

    // Secret never prefilled — password input should be empty
    const secretInput = screen.getByPlaceholderText("Leave blank to keep current");
    expect(secretInput).toBeInTheDocument();
    expect((secretInput as HTMLInputElement).value).toBe("");

    // Secret value must never appear in the DOM
    expect(screen.queryByText("SUPER_SECRET_VALUE")).not.toBeInTheDocument();
    expect(screen.queryByText(/secret_key/i)).not.toBeInTheDocument();

    // Fixed fields: bucket and key_prefix rendered as disabled inputs
    const bucketInput = screen.getByDisplayValue("my-data-bucket");
    expect(bucketInput).toBeDisabled();
    const keyPrefixInput = screen.getByDisplayValue("warehouse/eng");
    expect(keyPrefixInput).toBeDisabled();

    // The provider input should also be present and disabled
    const providerInput = screen.getByDisplayValue("s3");
    expect(providerInput).toBeDisabled();

    // Save button exists
    expect(screen.getByRole("button", { name: /save/i })).toBeInTheDocument();

    // Re-point warning must be visible
    expect(
      screen.getByText(/re-points this workspace.s catalog warehouse/i),
    ).toBeInTheDocument();
  });

  it("calls useUpdateStorage mutate with correct body when saved (secret left blank → null)", async () => {
    renderAt();

    // Change the endpoint
    const endpointInput = screen.getByDisplayValue("https://s3.example.com");
    fireEvent.change(endpointInput, { target: { value: "https://minio.corp.local" } });

    // Leave secret blank → secret_key should be null
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(mutateMock).toHaveBeenCalledWith({
        provider: "s3",
        bucket: "my-data-bucket",
        key_prefix: "warehouse/eng",
        endpoint: "https://minio.corp.local",
        region: "us-east-1",
        access_key: "AKIAIOSFODNN7EXAMPLE",
        secret_key: null,
        path_style: false,
      });
    });
  });

  it("passes secret_key when the user types a new secret", async () => {
    renderAt();

    const secretInput = screen.getByPlaceholderText("Leave blank to keep current");
    fireEvent.change(secretInput, { target: { value: "newS3cr3t!" } });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(mutateMock).toHaveBeenCalledWith(
        expect.objectContaining({ secret_key: "newS3cr3t!" }),
      );
    });
  });

  it("shows success message and clears secret field after successful save", async () => {
    renderAt();

    const secretInput = screen.getByPlaceholderText(
      "Leave blank to keep current",
    ) as HTMLInputElement;
    fireEvent.change(secretInput, { target: { value: "tempSecret" } });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(screen.getByText("Storage updated.")).toBeInTheDocument();
    });

    // Secret field must be cleared after save
    expect(secretInput.value).toBe("");
  });

  it("shows error message from the API on mutation failure", async () => {
    mutateMock.mockRejectedValue({
      message: "Invalid credentials: access denied by S3",
    });

    renderAt();

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(
        screen.getByText("Invalid credentials: access denied by S3"),
      ).toBeInTheDocument();
    });
  });

  it("shows 'A secret is currently configured' hint when has_secret is true", () => {
    renderAt();
    expect(screen.getByText("A secret is currently configured.")).toBeInTheDocument();
  });

  it("omits the secret hint when has_secret is false", () => {
    storageMock.mockReturnValue({
      data: { ...mockStorageInfo, has_secret: false },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(
      screen.queryByText("A secret is currently configured."),
    ).not.toBeInTheDocument();
  });

  it("sends endpoint as null when the endpoint field is cleared", async () => {
    renderAt();

    const endpointInput = screen.getByDisplayValue("https://s3.example.com");
    fireEvent.change(endpointInput, { target: { value: "" } });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(mutateMock).toHaveBeenCalledWith(
        expect.objectContaining({ endpoint: null }),
      );
    });
  });

  it("falls back to default key-prefix display when key_prefix is null", () => {
    storageMock.mockReturnValue({
      data: { ...mockStorageInfo, key_prefix: null },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();

    // Disabled key-prefix input should show the computed default
    const kpInput = screen.getByDisplayValue("workspaces/eng/warehouse");
    expect(kpInput).toBeDisabled();
  });

  // ── 404 (platform default) ────────────────────────────────────────────────

  it("shows platform-default message + no-support note on 404 (no edit form)", () => {
    storageMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 404 },
    });
    renderAt();

    expect(
      screen.getByText("This workspace uses the platform default storage."),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Customizing storage for a default-backed workspace isn.t supported here yet/i,
      ),
    ).toBeInTheDocument();

    // No edit form / inputs should be present
    expect(screen.queryByRole("button", { name: /save/i })).not.toBeInTheDocument();
    expect(
      screen.queryByPlaceholderText("Leave blank to keep current"),
    ).not.toBeInTheDocument();

    // Not a scary error — these alerts must not appear
    expect(
      screen.queryByText("Failed to load storage configuration"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/requires workspace-admin/i)).not.toBeInTheDocument();
  });

  // ── 403 ───────────────────────────────────────────────────────────────────

  it("shows workspace-admin notice on 403", () => {
    storageMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();

    expect(
      screen.getByText("Requires workspace-admin to view storage configuration."),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("This workspace uses the platform default storage."),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /save/i })).not.toBeInTheDocument();
  });

  // ── Generic error ─────────────────────────────────────────────────────────

  it("shows generic error alert on unexpected error status", () => {
    storageMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 500 },
    });
    renderAt();

    expect(screen.getByText("Failed to load storage configuration")).toBeInTheDocument();
  });
});
