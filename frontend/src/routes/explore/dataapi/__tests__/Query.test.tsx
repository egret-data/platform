import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// No msw in this repo — mock the API module directly.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/dataApi");

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import * as dataApi from "@/api/dataApi";
import { ParamLocation, ParamType, type DataApiEndpointDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query from "@/routes/explore/dataapi/detail/Query";

// Copied verbatim from `EndpointDetail.test.tsx` (near the top of that file)
// rather than imported — that file does not export it, and duplicating a
// fixture builder is cheaper than making a 900-line test file into a module.
function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    sql: "SELECT 1",
    params: [],
    output: {},
    cache: {},
    limits: {},
    rate_limit: {},
    source_kind: "sql",
    source_ref: null,
    live_version_id: null,
    state: "draft",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

function renderQuery(endpoint: DataApiEndpointDto = makeEndpoint()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/ws1/data-api/endpoints/${endpoint.id}`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<Query />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Query section — preview", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as never);
  });

  it("sends the on-screen SQL, not the loaded draft", async () => {
    const endpoint = makeEndpoint({ sql: "SELECT 1" });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.clear(sql);
    await userEvent.type(sql, "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: /preview/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 2");
  });

  it("says the result was capped when the engine truncated it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({
        status: 200,
        data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: true },
      }),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // A capped preview read as a small result set is the failure this exists to
    // stop, so the copy must name the cap.
    expect(await screen.findByText(/first 100 rows/i)).toBeInTheDocument();
  });

  it("shows the engine's own error text inline", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("line 1:8: mismatched input 'FRUM'")),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // Inline, not a toast: a toast vanishes before an author mid-edit reads it,
    // and this is exactly the moment they need the message on screen.
    expect(await screen.findByText(/mismatched input 'FRUM'/)).toBeInTheDocument();
  });

  it("says preview shows the engine's column names, not the published contract's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.4.1: say which is which or the two get confused.
    expect(await screen.findByText(/column names come from the engine/i)).toBeInTheDocument();
  });

  it("says a green preview does not prove a consumer can call it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.1: "The consequence must reach the UI, not just this document."
    expect(await screen.findByText(/runs as you/i)).toBeInTheDocument();
  });
});

describe("Query section — layout", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  it("holds the database picker, the SQL editor and the parameter declarations", async () => {
    const endpoint = makeEndpoint({
      params: [
        {
          name: "region",
          type: ParamType.string,
          location: ParamLocation.query,
          required: true,
          description: "",
        },
      ],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);

    renderQuery(endpoint);
    expect(await screen.findByLabelText("Database")).toBeInTheDocument();
    expect(screen.getByLabelText("SQL")).toBeInTheDocument();
    // The DECLARATION, from ParamEditor…
    expect(screen.getByDisplayValue("region")).toBeInTheDocument();
    // …and a separate test-value input for the same parameter, which is the
    // distinction merging the two would destroy.
    expect(screen.getByLabelText(/^region/)).toBeInTheDocument();
  });

  it("does not hold name, path or the config tables — those are Config's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Path")).not.toBeInTheDocument();
    expect(screen.queryByText("Rate limit")).not.toBeInTheDocument();
  });
});
