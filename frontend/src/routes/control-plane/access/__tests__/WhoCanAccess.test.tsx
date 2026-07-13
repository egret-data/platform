import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import WhoCanAccess from "@/routes/control-plane/access/WhoCanAccess";
import * as whoCanAccessApi from "@/api/whoCanAccess";
import * as workspaceAccessApi from "@/api/workspaceAccess";
import type { WhoCanAccess as WhoCanAccessData } from "@/api/whoCanAccess";

vi.mock("@/api/whoCanAccess");
vi.mock("@/api/workspaceAccess");
vi.mock("@/api/generated/admin-workspaces/admin-workspaces", () => ({
  adminWorkspacesListWs: vi.fn(() =>
    Promise.resolve({
      status: 200,
      data: [{ id: "ws1", slug: "demo", name: "Demo", status: "active" }],
      headers: new Headers(),
    }),
  ),
}));

const FULL: WhoCanAccessData = {
  object: { kind: "chart", id: "abc", name: "Revenue chart", workspace_slug: "demo" },
  entries: [
    {
      principal_type: "group",
      principal_id: "analysts",
      principal_name: "Analysts",
      access: "viewer",
      effect: "allow",
      source: "direct",
    },
    {
      principal_type: "user",
      principal_id: "bob-id",
      principal_name: "Bob",
      access: "ws_editor",
      effect: "allow",
      source: "workspace:demo",
    },
    {
      principal_type: "user",
      principal_id: "root-id",
      principal_name: "Root",
      access: "platform_admin",
      effect: "allow",
      source: "platform",
    },
  ],
};

/** Render the page deep-linked at the given query string. */
function renderPage(query = "?ws=demo&kind=chart&id=abc") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/who-can-access${query}`]}>
        <Routes>
          <Route path="/control-plane/who-can-access" element={<WhoCanAccess />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("WhoCanAccess", () => {
  beforeEach(() => {
    vi.mocked(whoCanAccessApi.getWhoCanAccess).mockReset();
    vi.mocked(whoCanAccessApi.getWhoCanAccess).mockResolvedValue(FULL);
    vi.mocked(workspaceAccessApi.useWorkspaceShares).mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    } as unknown as ReturnType<typeof workspaceAccessApi.useWorkspaceShares>);
  });

  test("renders the object header and every principal with access, effect, and source", async () => {
    renderPage();

    // Object header.
    expect(await screen.findByText("Revenue chart")).toBeInTheDocument();
    expect(screen.getByText("chart")).toBeInTheDocument();
    expect(screen.getByText("/demo")).toBeInTheDocument();

    // Direct group grant.
    expect(screen.getByText("Analysts")).toBeInTheDocument();
    expect(screen.getByText("viewer")).toBeInTheDocument();

    // Workspace-role user grant.
    expect(screen.getByText("Bob")).toBeInTheDocument();
    expect(screen.getByText("ws_editor")).toBeInTheDocument();
    expect(screen.getByText(/workspace demo/i)).toBeInTheDocument();

    // Platform admin.
    expect(screen.getByText("Root")).toBeInTheDocument();
    expect(screen.getByText("platform admin")).toBeInTheDocument();

    // Every entry shows an allow badge.
    expect(screen.getAllByText("allow").length).toBe(3);
  });

  test("renders a deny badge for a deny entry", async () => {
    vi.mocked(whoCanAccessApi.getWhoCanAccess).mockResolvedValue({
      object: FULL.object,
      entries: [
        {
          principal_type: "user",
          principal_id: "carol-id",
          principal_name: "Carol",
          access: "viewer",
          effect: "deny",
          source: "direct",
        },
      ],
    });

    renderPage();

    expect(await screen.findByText("Carol")).toBeInTheDocument();
    expect(screen.getByText("deny")).toBeInTheDocument();
  });

  test("shows the object picker prompt when no kind/id is selected", () => {
    renderPage("");

    expect(screen.getByText(/Pick an object to see who can access it\./i)).toBeInTheDocument();
    expect(whoCanAccessApi.getWhoCanAccess).not.toHaveBeenCalled();
  });

  test("shows the role:read permission alert on 403", async () => {
    vi.mocked(whoCanAccessApi.getWhoCanAccess).mockRejectedValue(
      Object.assign(new Error("forbidden"), { status: 403 }),
    );

    renderPage();

    expect(await screen.findByText(/role:read/i)).toBeInTheDocument();
  });
});
