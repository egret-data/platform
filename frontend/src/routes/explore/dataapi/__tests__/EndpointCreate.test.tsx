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
    await userEvent.type(screen.getByLabelText("Description (optional)"), "Yesterday's orders");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    // Two databases in the picker, so choosing the SECOND proves the value is
    // read from the choice rather than defaulted to the only option available.
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "lake" }));
    // Method is picked AWAY from its default for the same reason the second
    // database is: asserting `method: "GET"` on a form that defaults to GET
    // passes just as well for a page that ignores the picker, or has no picker
    // at all. POST is the only value that can only come from the control.
    await userEvent.click(screen.getByRole("combobox", { name: "Method" }));
    await userEvent.click(await screen.findByRole("option", { name: "POST" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    const body = mutateAsync.mock.calls[0][0].data;
    expect(body).toMatchObject({
      name: "Daily report",
      path: "reports/daily",
      database_id: "db-2",
      method: "POST",
      description: "Yesterday's orders",
    });
    // ONE statement, named `main` — the shape migration 0129 gave every
    // endpoint that pre-dated the statement list, so a new endpoint and a
    // migrated one are the same shape from here on. A deep-equal, not a length
    // check: a page that sent a rows/total pair, or that left the name blank
    // (which the server refuses with a 400), passes any weaker assertion.
    expect(body.statements).toEqual([{ name: "main", sql: "SELECT 1" }]);
    // The create page must not invent config the editor owns.
    expect(Object.keys(body)).not.toContain("cache");
    expect(Object.keys(body)).not.toContain("rate_limit");
  });

  // The other half of BOTH optional-ish fields.
  //
  // `description`: blank must mean ABSENT, not `""` — a page that always sent
  // the key would pass the test above.
  //
  // `method`: this test never touches the Method picker, so it pins the
  // DEFAULT. Asserting only the non-default value (POST, above) turns out not
  // to be enough — it just moves which constant is unfalsifiable: hard-coding
  // `method: "POST"` in the request body passes a suite that only ever picks
  // POST. Measured, not assumed: that exact mutation survived until this line
  // existed. With both pinned, only reading the control passes.
  it("leaves description out and method at GET when the author touches neither", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    const body = mutateAsync.mock.calls[0][0].data;
    expect(Object.keys(body)).not.toContain("description");
    expect(body).toMatchObject({ method: "GET" });
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
