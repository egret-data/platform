import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────

const roleCatalogMock = vi.fn();

vi.mock("@/api/workspaceAccess", () => ({
  useWorkspaceRoleCatalog: (...args: unknown[]) => roleCatalogMock(...args),
}));

import RolesPermissions from "@/routes/explore/settings/RolesPermissions";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderPage() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/settings/roles"]}>
        <Routes>
          <Route path="/explore/:workspace/settings/roles" element={<RolesPermissions />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const viewerRole = {
  id: "r-viewer",
  name: "ws_viewer",
  description: "Read-only access to workspace content.",
  parent_role_id: null,
  permissions: [
    { key: "ws:read", description: "View workspace contents", group: "Workspace" },
  ],
};

const adminRole = {
  id: "r-admin",
  name: "ws_admin",
  description: "Full administrative control of the workspace.",
  parent_role_id: "r-viewer",
  permissions: [
    { key: "ws:read", description: "View workspace contents", group: "Workspace" },
    { key: "ws:manage_members", description: "Add or remove workspace members", group: "Workspace" },
  ],
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  roleCatalogMock.mockReset();
  roleCatalogMock.mockReturnValue({
    data: [adminRole, viewerRole],
    isLoading: false,
    isError: false,
    error: null,
  });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("RolesPermissions page", () => {
  it("renders roles, permissions, group, and the resolved parent role name — with no edit controls", () => {
    renderPage();

    // Roles are listed as (collapsed) accordion rows.
    expect(screen.getByText("ws_admin")).toBeInTheDocument();
    expect(screen.getByText("ws_viewer")).toBeInTheDocument();

    // "Inherits from" badge on ws_admin resolves to the parent's name (ws_viewer).
    const inheritsBadge = screen.getByText(/inherits from/i);
    expect(inheritsBadge).toHaveTextContent("ws_viewer");

    // Expanding a role reveals its permissions + group.
    fireEvent.click(screen.getByRole("button", { name: /ws_admin/i }));
    expect(screen.getByText("ws:manage_members")).toBeInTheDocument();
    expect(screen.getAllByText("Workspace").length).toBeGreaterThan(0);

    // Read-only: no edit/save/add controls anywhere on the page.
    expect(screen.queryByRole("button", { name: /edit|save|add/i })).not.toBeInTheDocument();
  });

  it("shows a 403-forbidden message for non-admins", () => {
    roleCatalogMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403, message: "Forbidden" },
    });
    renderPage();

    expect(screen.getByRole("alert")).toHaveTextContent(/only workspace admins/i);
  });

  it("shows a skeleton while the role catalog is loading", () => {
    roleCatalogMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderPage();

    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });
});
