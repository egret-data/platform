import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <PermissionsPage />
    </QueryClientProvider>,
  );
}

describe("PermissionsPage", () => {
  test("renders group headings, permission keys, and role badges", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    renderPage();

    expect(await screen.findByText("Users")).toBeInTheDocument();
    expect(screen.getByText("Policies")).toBeInTheDocument();
    expect(screen.getByText("users.read")).toBeInTheDocument();
    expect(screen.getByText("policies.read")).toBeInTheDocument();
    expect(screen.getAllByText("Admin").length).toBeGreaterThan(0);
    expect(screen.getByText("Analyst")).toBeInTheDocument();
  });

  test("renders a muted dash when a permission has no roles granted", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(GROUPS);
    renderPage();

    await screen.findByText("users.write");
    // The row for users.write should show a "—" placeholder rather than a badge.
    const row = screen.getByText("users.write").closest("tr");
    expect(row).not.toBeNull();
    expect(row!.textContent).toContain("—");
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
  test("filters permissions across grouped cards and restores when cleared", async () => {
    vi.mocked(api.listPermissions).mockResolvedValue(SEARCH_GROUPS);
    const user = userEvent.setup();
    renderPage();

    // Initially every permission renders across both groups.
    expect(await screen.findByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("chart:manage")).toBeInTheDocument();
    expect(screen.getByText("engine:read")).toBeInTheDocument();
    expect(screen.getByText("Engines")).toBeInTheDocument();

    const box = screen.getByPlaceholderText("Search permissions…");

    // Typing "chart" keeps the chart permissions and hides the Engines group.
    await user.type(box, "chart");
    expect(screen.getByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("chart:manage")).toBeInTheDocument();
    expect(screen.queryByText("engine:read")).not.toBeInTheDocument();
    expect(screen.queryByText("Engines")).not.toBeInTheDocument();

    // A nonsense query matches nothing → the no-matches message shows.
    await user.clear(box);
    await user.type(box, "zzzznope");
    expect(screen.queryByText("chart:read")).not.toBeInTheDocument();
    expect(screen.getByText(/No permissions match/)).toBeInTheDocument();

    // Clearing the search restores every permission.
    await user.clear(box);
    expect(screen.getByText("chart:read")).toBeInTheDocument();
    expect(screen.getByText("engine:read")).toBeInTheDocument();
    expect(screen.getByText("Engines")).toBeInTheDocument();
  });
});
