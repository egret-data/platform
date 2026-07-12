import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import AccessExplorer from "@/routes/control-plane/access/AccessExplorer";
import * as accessApi from "@/api/access";
import * as policiesApi from "@/api/policies";
import type { EffectiveAccess } from "@/api/access";

vi.mock("@/api/access");
vi.mock("@/api/policies");

const FULL_ACCESS: EffectiveAccess = {
  principal: { type: "user", id: "alice-id", name: "Alice", is_platform_admin: false },
  platform_roles: [{ name: "data_analyst", source: "direct" }],
  permissions: [
    { key: "catalog.read", via: [{ role: "data_analyst", inherited_from: "data_steward" }] },
  ],
  workspaces: [
    {
      workspace_id: "ws1",
      workspace_slug: "analytics",
      workspace_name: "Analytics Team",
      role: "ws_editor",
      source: "direct",
    },
  ],
  objects: [
    {
      kind: "dashboard",
      id: "d1",
      name: "Revenue",
      workspace_slug: "analytics",
      role: "viewer",
      effect: "allow",
      source: "direct",
    },
  ],
  denies: [{ scope: "workspace", target: "Finance", role: "ws_editor", source: "direct" }],
};

/** Render the page deep-linked at `?user=<id>` (or any query string). */
function renderPage(query = "?user=alice-id") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/access-explorer${query}`]}>
        <Routes>
          <Route path="/control-plane/access-explorer" element={<AccessExplorer />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("AccessExplorer", () => {
  beforeEach(() => {
    vi.mocked(policiesApi.listPrincipals).mockResolvedValue([]);
    vi.mocked(accessApi.getEffectiveAccess).mockResolvedValue(FULL_ACCESS);
  });

  test("renders roles, permissions with provenance, workspaces, and a highlighted deny", async () => {
    renderPage();

    // Principal header.
    expect(await screen.findByText("Alice")).toBeInTheDocument();

    // Platform role chip.
    expect(screen.getByText("data_analyst")).toBeInTheDocument();

    // Permission provenance: "data_analyst (inherited from data_steward)".
    expect(screen.getByText(/inherited from data_steward/i)).toBeInTheDocument();

    // Workspace membership.
    expect(screen.getByText("Analytics Team")).toBeInTheDocument();

    // Denies / Blocked section: heading + target.
    expect(screen.getByRole("heading", { name: "Blocked" })).toBeInTheDocument();
    expect(screen.getByText("Finance")).toBeInTheDocument();
  });

  test("preselects the principal from ?user= and calls getEffectiveAccess", async () => {
    renderPage("?user=alice-id");

    await waitFor(() =>
      expect(accessApi.getEffectiveAccess).toHaveBeenCalledWith("user", "alice-id"),
    );
  });

  test("shows the Superuser note and no permission table for a platform-admin principal", async () => {
    vi.mocked(accessApi.getEffectiveAccess).mockResolvedValue({
      ...FULL_ACCESS,
      principal: { type: "user", id: "root-id", name: "Root", is_platform_admin: true },
      permissions: [],
    });

    renderPage("?user=root-id");

    // Superuser badge + note render.
    expect(await screen.findByText("Superuser")).toBeInTheDocument();
    expect(
      screen.getByText(/Bypasses all permission checks — effectively has every permission\./i),
    ).toBeInTheDocument();

    // No effective-permissions table for a superuser.
    expect(screen.queryByText("Effective permissions")).not.toBeInTheDocument();
    expect(screen.queryByText(/inherited from/i)).not.toBeInTheDocument();
  });
});
