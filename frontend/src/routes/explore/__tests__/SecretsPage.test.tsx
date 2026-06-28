import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────
const secretsMock = vi.fn();
const createMutateAsync = vi.fn();
const createMutation = vi.fn();
const updateMutateAsync = vi.fn();
const updateMutation = vi.fn();
const deleteMutateAsync = vi.fn();
const deleteMutation = vi.fn();

vi.mock("@/api/secrets", () => ({
  useSecrets: () => secretsMock(),
  useCreateSecret: () => createMutation(),
  useUpdateSecret: () => updateMutation(),
  useDeleteSecret: () => deleteMutation(),
}));

import SecretsPage from "@/routes/explore/SecretsPage";

// ── Render helper ─────────────────────────────────────────────────────────────
function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/secrets"]}>
        <Routes>
          <Route path="/explore/:workspace/secrets" element={<SecretsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────
const mockSecret1 = {
  id: "sec-1",
  name: "prod-s3-secret",
  description: "S3 access for prod",
  created_at: "2025-01-01T00:00:00Z",
  used_by: 2,
};

const mockSecret2 = {
  id: "sec-2",
  name: "unused-secret",
  description: null,
  created_at: "2025-02-01T00:00:00Z",
  used_by: 0,
};

// ── Setup ─────────────────────────────────────────────────────────────────────
beforeEach(() => {
  secretsMock.mockReset();
  createMutation.mockReset();
  updateMutation.mockReset();
  deleteMutation.mockReset();
  createMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  deleteMutateAsync.mockReset();

  secretsMock.mockReturnValue({
    data: [mockSecret1, mockSecret2],
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
    mutateAsync: deleteMutateAsync,
    isPending: false,
  });

  createMutateAsync.mockResolvedValue(mockSecret1);
  updateMutateAsync.mockResolvedValue(mockSecret1);
  deleteMutateAsync.mockResolvedValue(undefined);
});

describe("SecretsPage", () => {
  // ── List rendering ────────────────────────────────────────────────────────

  it("renders the secrets list with name, description, and used_by count", () => {
    renderAt();

    expect(screen.getByText("prod-s3-secret")).toBeInTheDocument();
    expect(screen.getByText("S3 access for prod")).toBeInTheDocument();
    // used_by counts are shown as badges
    expect(screen.getByText("2")).toBeInTheDocument();

    expect(screen.getByText("unused-secret")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
  });

  it("never renders a secret value in the table", () => {
    renderAt();
    // The DTO carries no value; ensure nothing leaks.
    expect(screen.queryByText(/SUPER_SECRET/i)).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue(/SUPER_SECRET/i)).not.toBeInTheDocument();
  });

  it("shows empty state when no secrets exist", () => {
    secretsMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText("No secrets yet.")).toBeInTheDocument();
  });

  it("shows skeleton while loading", () => {
    secretsMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.queryByText("prod-s3-secret")).not.toBeInTheDocument();
  });

  // ── 403 ───────────────────────────────────────────────────────────────────

  it("shows workspace-admin notice on 403", () => {
    secretsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();

    expect(
      screen.getByText("Requires workspace-admin to manage secrets."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add secret")).not.toBeInTheDocument();
  });

  it("shows generic error alert on unexpected error status", () => {
    secretsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 500 },
    });
    renderAt();

    expect(screen.getByText("Failed to load secrets")).toBeInTheDocument();
  });

  // ── Create ────────────────────────────────────────────────────────────────

  it("creates a secret with { name, value } on Add submit", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add secret/i }));

    await waitFor(() => screen.getByLabelText(/^value$/i));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "new-secret" },
    });
    fireEvent.change(screen.getByLabelText(/^value$/i), {
      target: { value: "s3cr3t" },
    });

    fireEvent.click(screen.getByRole("button", { name: /add secret/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ name: "new-secret", value: "s3cr3t" }),
      );
    });
  });

  // ── Edit / rotate ─────────────────────────────────────────────────────────

  it("rotates the value on edit (PUT carries the new value)", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit secret"));

    // Name is prefilled; value field is blank.
    expect(screen.getByDisplayValue("prod-s3-secret")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText(/new value/i), {
      target: { value: "rotated-value" },
    });

    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "sec-1",
          input: expect.objectContaining({ value: "rotated-value" }),
        }),
      );
    });
  });

  it("keeps the existing value on edit when the value field is left blank", async () => {
    renderAt();

    const editButtons = screen.getAllByRole("button", { name: /^edit/i });
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText("Edit secret"));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "renamed-secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const call = updateMutateAsync.mock.calls[0][0];
      expect(call.input.name).toBe("renamed-secret");
      expect(call.input.value).toBeUndefined();
    });
  });

  // ── Delete ────────────────────────────────────────────────────────────────

  it("disables Delete for an in-use secret with an explanatory title", () => {
    renderAt();

    const delBtn = screen.getByRole("button", { name: /delete prod-s3-secret/i });
    expect(delBtn).toBeDisabled();
    expect(delBtn).toHaveAttribute("title", "In use by 2 connections");
  });

  it("deletes an unused secret on confirm", async () => {
    renderAt();

    fireEvent.click(screen.getByRole("button", { name: /delete unused-secret/i }));

    await waitFor(() => {
      expect(screen.getByText(/Delete «unused-secret»/)).toBeInTheDocument();
    });

    // AlertDialog action button.
    const confirmBtn = screen.getAllByRole("button", { name: /^delete$/i }).pop()!;
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith("sec-2");
    });
  });

  it("surfaces a 409 'in use' error from the server in the delete dialog", async () => {
    deleteMutateAsync.mockRejectedValue({
      status: 409,
      message: "in use by 3 connections",
    });
    renderAt();

    fireEvent.click(screen.getByRole("button", { name: /delete unused-secret/i }));

    await waitFor(() => screen.getByText(/Delete «unused-secret»/));

    const confirmBtn = screen.getAllByRole("button", { name: /^delete$/i }).pop()!;
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(screen.getByText("in use by 3 connections")).toBeInTheDocument();
    });
  });
});
