import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const storageMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWorkspaceStorage: () => storageMock(),
}));

import StorageSettings from "../StorageSettings";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/settings/storage"]}>
        <Routes>
          <Route path="/explore/:workspace/settings/storage" element={<StorageSettings />} />
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
  storageMock.mockReturnValue({
    data: mockStorageInfo,
    isLoading: false,
    isError: false,
    error: null,
  });
});

describe("StorageSettings", () => {
  it("renders storage info fields and shows 'Configured' badge when has_secret is true", () => {
    renderAt();

    expect(screen.getByText("s3")).toBeInTheDocument();
    expect(screen.getByText("my-data-bucket")).toBeInTheDocument();
    expect(screen.getByText("https://s3.example.com")).toBeInTheDocument();
    expect(screen.getByText("us-east-1")).toBeInTheDocument();
    expect(screen.getByText("AKIAIOSFODNN7EXAMPLE")).toBeInTheDocument();
    expect(screen.getByText("warehouse/eng")).toBeInTheDocument();

    // has_secret:true → badge says "Configured"
    expect(screen.getByText("Configured")).toBeInTheDocument();
    expect(screen.queryByText("Not set")).not.toBeInTheDocument();

    // Sanity: no actual secret value in the DOM (the type has none — assert a
    // known fake secret string is not rendered).
    expect(screen.queryByText("SUPER_SECRET_VALUE")).not.toBeInTheDocument();
    expect(screen.queryByText(/secret_key/i)).not.toBeInTheDocument();
  });

  it("shows 'Not set' badge when has_secret is false", () => {
    storageMock.mockReturnValue({
      data: { ...mockStorageInfo, has_secret: false },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();

    expect(screen.getByText("Not set")).toBeInTheDocument();
    expect(screen.queryByText("Configured")).not.toBeInTheDocument();
  });

  it("falls back to 'AWS default' when endpoint is null", () => {
    storageMock.mockReturnValue({
      data: { ...mockStorageInfo, endpoint: null },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();

    expect(screen.getByText("AWS default")).toBeInTheDocument();
  });

  it("falls back to default key-prefix label when key_prefix is null", () => {
    storageMock.mockReturnValue({
      data: { ...mockStorageInfo, key_prefix: null },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();

    expect(screen.getByText(/default \(workspaces\/eng\/warehouse\)/)).toBeInTheDocument();
  });

  it("shows the platform-default-storage message on 404 (not an error state)", () => {
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

    // Not a scary error — these alerts must not be present.
    expect(screen.queryByText("Failed to load storage configuration")).not.toBeInTheDocument();
    expect(screen.queryByText(/requires workspace-admin/i)).not.toBeInTheDocument();
  });

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
    expect(screen.queryByText("This workspace uses the platform default storage.")).not.toBeInTheDocument();
  });

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
