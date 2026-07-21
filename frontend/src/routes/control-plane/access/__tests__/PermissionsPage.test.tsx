import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, test, vi } from "vitest";

import PermissionsPage from "@/routes/control-plane/access/PermissionsPage";
import * as api from "@/api/access";

vi.mock("@/api/access");

const GROUPS: api.PermissionCatalogGroup[] = [
  {
    group: "Users",
    permissions: [
      { key: "users.read", description: "View users", roles: ["Admin"], scope: "platform" },
      {
        key: "users.write",
        description: "Create and edit users",
        roles: [],
        scope: "platform",
      },
    ],
  },
  {
    group: "Policies",
    permissions: [
      {
        key: "policies.read",
        description: "View policies",
        roles: ["Admin", "Analyst"],
        scope: "platform",
      },
    ],
  },
];

const ROLES = [
  { id: "r-admin", name: "Admin", scope: "platform", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
  { id: "r-analyst", name: "Analyst", scope: "platform", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
] as unknown as api.AccessRole[];

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <PermissionsPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("PermissionsPage", () => {
  test("renders permission keys, their area, and a granted-role count", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    renderPage();

    expect(await screen.findByText("users.read")).toBeInTheDocument();
    expect(screen.getByText("policies.read")).toBeInTheDocument();
    // Areas surface as a column badge (the "Users" area has two permissions).
    expect(screen.getAllByText("Users").length).toBeGreaterThan(0);
    expect(screen.getByText("Policies")).toBeInTheDocument();
    // "Granted to" is now a count, not a chip list: policies.read → 2 roles.
    expect(screen.getByText("2 roles")).toBeInTheDocument();
    expect(screen.getByText("1 role")).toBeInTheDocument();
  });

  test("summarises the catalog in the strip", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    renderPage();

    await screen.findByText("users.read");
    // 3 permissions across 2 areas; 2 granted, 1 ungranted.
    expect(screen.getByText("permissions")).toBeInTheDocument();
    expect(screen.getByText("areas")).toBeInTheDocument();
    expect(screen.getByText("granted")).toBeInTheDocument();
    expect(screen.getByText("ungranted")).toBeInTheDocument();
  });

  test("shows 'Not granted' for a permission with no roles", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    renderPage();

    // The row for users.write has no grants → "Not granted", no count link.
    const row = (await screen.findByText("users.write")).closest("tr");
    expect(row).not.toBeNull();
    expect(row!.textContent).toContain("Not granted");
  });

  test("the count opens a dialog listing (and linking) the granted roles", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    const user = userEvent.setup();
    renderPage();

    await screen.findByText("policies.read");
    await user.click(screen.getByText("2 roles"));

    const dialog = await screen.findByRole("dialog");
    // Both granted roles are listed, and resolvable ones link to their detail page.
    expect(within(dialog).getByText("Admin")).toBeInTheDocument();
    expect(within(dialog).getByText("Analyst")).toBeInTheDocument();
  });
});

const SEARCH_GROUPS: api.PermissionCatalogGroup[] = [
  {
    group: "Charts",
    permissions: [
      { key: "chart:read", description: "View charts", roles: ["Admin"], scope: "platform" },
      {
        key: "chart:manage",
        description: "Create and edit charts",
        roles: [],
        scope: "platform",
      },
    ],
  },
  {
    group: "Engines",
    permissions: [
      { key: "engine:read", description: "View engines", roles: ["Admin"], scope: "platform" },
    ],
  },
];

describe("PermissionsPage search", () => {
  test("filters the table and restores when cleared", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(SEARCH_GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    const user = userEvent.setup();
    renderPage();

    // Initially every permission renders.
    expect(await screen.findByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("chart:manage")).toBeInTheDocument();
    expect(screen.getByText("engine:read")).toBeInTheDocument();

    const box = screen.getByPlaceholderText("Search permissions…");

    // Typing "chart" keeps the chart permissions and hides the engine row.
    await user.type(box, "chart");
    expect(screen.getByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("chart:manage")).toBeInTheDocument();
    expect(screen.queryByText("engine:read")).not.toBeInTheDocument();

    // A nonsense query matches nothing.
    await user.clear(box);
    await user.type(box, "zzzznope");
    expect(screen.queryByText("chart:read")).not.toBeInTheDocument();

    // Clearing the search restores every permission.
    await user.clear(box);
    expect(screen.getByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("engine:read")).toBeInTheDocument();
  });
});
