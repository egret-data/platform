import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/api/dataApi");
vi.mock("@/api/databases");

import * as dataApi from "@/api/dataApi";
import * as databasesApi from "@/api/databases";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";

const navigate = vi.fn();
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => navigate,
}));

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/ws1/data-api/endpoints/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/new" element={<EndpointCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("EndpointCreate", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }, { id: "db-2", name: "lake" }],
    } as never);
  });

  it("sends what the author typed, and only the fields a row needs", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    // Two databases in the picker, so choosing the SECOND proves the value is
    // read from the choice rather than defaulted to the only option available.
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "lake" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    const body = mutateAsync.mock.calls[0][0].data;
    expect(body).toMatchObject({ name: "Daily report", path: "reports/daily", sql: "SELECT 1", database_id: "db-2" });
    // The create page must not invent config the editor owns.
    expect(Object.keys(body)).not.toContain("cache");
    expect(Object.keys(body)).not.toContain("rate_limit");
  });

  it("hands off to the created endpoint's editor, not back to the list", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } }),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "x");
    await userEvent.type(screen.getByLabelText("Path"), "x");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));
    expect(navigate).toHaveBeenCalledWith("/explore/ws1/data-api/endpoints/e-9");
  });

  it("keeps what the author typed when creation fails", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("path already taken")),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    // A dialog could be dismissed and retried; a page that clears itself on
    // failure loses work the author cannot get back. The engine's own message
    // must be on screen, and every field must still hold its value.
    expect(await screen.findByText(/path already taken/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("Daily report");
    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT 1");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("renders exactly one h1", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderCreate();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
});
