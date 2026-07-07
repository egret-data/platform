import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, test, vi } from "vitest";

import PermissionsPage from "@/routes/control-plane/access/PermissionsPage";
import * as api from "@/api/access";

vi.mock("@/api/access");

const GROUPS: api.PermissionCatalogGroup[] = [
  {
    group: "Users",
    permissions: [
      { key: "users.read", description: "View users", roles: ["Admin"] },
      { key: "users.write", description: "Create and edit users", roles: [] },
    ],
  },
  {
    group: "Policies",
    permissions: [
      { key: "policies.read", description: "View policies", roles: ["Admin", "Analyst"] },
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
