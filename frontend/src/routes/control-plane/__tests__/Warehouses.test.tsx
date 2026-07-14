import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import Warehouses from "@/routes/control-plane/Warehouses";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";

vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

const WAREHOUSE: api.Warehouse = {
  id: "w1",
  name: "analytics",
  workspaceId: "ws1",
  storageConnId: "s1",
  lakekeeperWarehouseId: "lk-123",
};

function renderPage(entry = "/control-plane/warehouses") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[entry]}>
        <Warehouses />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Warehouses", () => {
  beforeEach(() => {
    vi.mocked(api.listStorage).mockResolvedValue([]);
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: [{ id: "ws1", slug: "eng", name: "Engineering" }],
    } as ReturnType<typeof hooks.useMyWorkspaces>);
  });

  test("renders warehouses with workspace name + provisioned badge", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([WAREHOUSE]);
    renderPage();
    expect(screen.getByText("Warehouses")).toBeInTheDocument();
    expect(await screen.findByText("analytics")).toBeInTheDocument();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    // "Provisioned" appears as both the column header and the status badge.
    expect(screen.getAllByText("Provisioned").length).toBeGreaterThanOrEqual(2);
  });

  test("shows the empty state", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText(/No warehouses yet/i)).toBeInTheDocument();
  });

  test("?workspace= filter scopes the list to one workspace + shows a filter banner", async () => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: [
        { id: "ws1", slug: "eng", name: "Engineering" },
        { id: "ws2", slug: "sales", name: "Sales" },
      ],
    } as ReturnType<typeof hooks.useMyWorkspaces>);
    vi.mocked(api.listWarehouses).mockResolvedValue([
      WAREHOUSE,
      { ...WAREHOUSE, id: "w2", name: "sales-wh", workspaceId: "ws2" },
    ]);
    renderPage("/control-plane/warehouses?workspace=ws1");

    // Only ws1's warehouse is shown; ws2's is filtered out.
    expect(await screen.findByText("analytics")).toBeInTheDocument();
    expect(screen.queryByText("sales-wh")).not.toBeInTheDocument();

    // Filter banner names the workspace and offers a clear link.
    expect(screen.getByText(/Filtered to workspace/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Show all workspaces/i })).toHaveAttribute(
      "href",
      "/control-plane/warehouses",
    );
  });

  test("opens the Add dialog", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /Add warehouse/i }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: /Add warehouse/i })).toBeInTheDocument(),
    );
  });
});
