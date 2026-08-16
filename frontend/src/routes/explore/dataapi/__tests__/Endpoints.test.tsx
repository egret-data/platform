import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// AuditLog.test.tsx). Endpoints.tsx also reads folders (grouping) and
// databases (the Engine column) unconditionally, and the download dialog
// always mounts and reads service accounts even while closed — all three
// need a mock or they fire a real, unhandled `fetch` in jsdom.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/generated/folders/folders");
vi.mock("@/api/databases");

import * as api from "@/api/generated/data-api/data-api";
import * as foldersApi from "@/api/generated/folders/folders";
import * as databasesApi from "@/api/databases";
import { ApiError } from "@/api/client";
import type { DataApiEndpointDto } from "@/api/generated/model";
import Endpoints from "@/routes/explore/dataapi/Endpoints";

function mockList(over: Partial<{ data: unknown; isError: boolean; isLoading: boolean }> = {}) {
  vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, ...over } as unknown as
      ReturnType<typeof api.useDataApiListEndpoints>,
  );
}

function mockFolders(folders: { id: string; name: string }[] = []) {
  vi.mocked(foldersApi.useFoldersFoldersList).mockReturnValue(
    { data: { status: 200, data: folders }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof foldersApi.useFoldersFoldersList>,
  );
}

const DB_TRINO = { id: "db-1", name: "warehouse", engine: "trino", host: "h", port: 1, options: {}, has_secret: true, status: "ok", delegated: false };

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    sql: "select 1",
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

const FOLDERED_ROW = makeEndpoint({
  id: "ep-1", name: "Daily report", folder_id: "f-1", state: "live", live_version_id: "v-1",
});
const LOOSE_ROW = makeEndpoint({
  id: "ep-2", name: "Ad hoc query", folder_id: null, state: "draft",
});

const SERVICE_ACCOUNT = { id: "sa-1", name: "reporting-bot", description: "", email: "sa@egret.local", created_at: "2026-08-01T00:00:00Z" };

const createMutateAsync = vi.fn();
const deleteMutateAsync = vi.fn();
const disableMutateAsync = vi.fn();
const enableMutateAsync = vi.fn();

function renderEndpoints() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints"]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints" element={<Endpoints />} />
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<p>endpoint detail pane</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();

  mockList({ data: { status: 200, data: [] } });
  mockFolders([]);

  vi.mocked(databasesApi.useDatabases).mockReturnValue(
    { data: [DB_TRINO], isLoading: false, isError: false } as unknown as ReturnType<typeof databasesApi.useDatabases>,
  );

  vi.mocked(api.useDataApiCreateEndpoint).mockReturnValue(
    { mutateAsync: createMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiCreateEndpoint>,
  );
  vi.mocked(api.useDataApiDeleteEndpoint).mockReturnValue(
    { mutateAsync: deleteMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiDeleteEndpoint>,
  );
  vi.mocked(api.useDataApiDisableEndpoint).mockReturnValue(
    { mutateAsync: disableMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiDisableEndpoint>,
  );
  vi.mocked(api.useDataApiEnableEndpoint).mockReturnValue(
    { mutateAsync: enableMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiEnableEndpoint>,
  );
  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: [SERVICE_ACCOUNT] }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
  vi.mocked(api.dataApiGetOpenapi).mockResolvedValue(
    { status: 200, data: { openapi: "3.0.3" }, headers: new Headers() } as unknown as
      Awaited<ReturnType<typeof api.dataApiGetOpenapi>>,
  );

  createMutateAsync.mockResolvedValue({ status: 201, data: { id: "ep-new" }, headers: new Headers() });
  deleteMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  disableMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  enableMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });

  // jsdom has no real download machinery; the download dialog's success path
  // touches it, so stub it out rather than let it throw/log "not implemented".
  global.URL.createObjectURL = vi.fn(() => "blob:mock");
  global.URL.revokeObjectURL = vi.fn();
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
});

describe("Endpoints", () => {
  it("shows an error state, not the first-run CTA, when the list fails", async () => {
    mockList({ isError: true });
    renderEndpoints();
    expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
    // The CTA copy that belongs to a genuinely EMPTY list must not appear on a failure.
    expect(screen.queryByText(/no endpoints yet/i)).not.toBeInTheDocument();
  });

  it("groups rows under their folder", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW, LOOSE_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    renderEndpoints();
    expect(await screen.findByText("Reporting")).toBeInTheDocument(); // folder header
    expect(screen.getByText("Uncategorized")).toBeInTheDocument();
  });

  it("shows the first-run CTA when the list is genuinely empty", async () => {
    mockList({ data: { status: 200, data: [] } });
    renderEndpoints();
    expect(await screen.findByText(/no endpoints yet/i)).toBeInTheDocument();
  });

  it("shows the loading skeleton while isLoading", () => {
    mockList({ isLoading: true });
    renderEndpoints();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("renders name, method+path, state badge, version, engine and last-edited for a row", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    renderEndpoints();
    expect(await screen.findByText("Daily report")).toBeInTheDocument();
    expect(screen.getByText("GET")).toBeInTheDocument();
    expect(screen.getByText("/reports/daily")).toBeInTheDocument();
    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(screen.getByText("Published")).toBeInTheDocument(); // has a live_version_id
    expect(screen.getByText("Trino")).toBeInTheDocument(); // resolved from database_id via useDatabases
  });

  it("shows a dash for the Version column when the endpoint has never been published", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    renderEndpoints();
    await screen.findByText("Ad hoc query");
    const row = screen.getByText("Ad hoc query").closest("tr")!;
    expect(row.textContent).toContain("—");
  });

  it("SummaryStrip 'live' chip filters the table to live rows only", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW, LOOSE_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Daily report");
    expect(screen.getByText("Ad hoc query")).toBeInTheDocument();

    await user.click(screen.getByText("live"));
    expect(screen.getByText("Daily report")).toBeInTheDocument();
    expect(screen.queryByText("Ad hoc query")).not.toBeInTheDocument();
  });

  it("filters rows by the search box", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW, LOOSE_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Daily report");
    await user.type(screen.getByPlaceholderText(/search endpoints/i), "Ad hoc");
    expect(screen.queryByText("Daily report")).not.toBeInTheDocument();
    expect(screen.getByText("Ad hoc query")).toBeInTheDocument();
  });

  it("creating an endpoint collects name/path/method/database and navigates to the new id", async () => {
    mockList({ data: { status: 200, data: [] } });
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /\+ endpoint/i }));
    await screen.findByText("New endpoint");

    await user.type(screen.getByLabelText("Name"), "Weekly rollup");
    await user.type(screen.getByLabelText("Path"), "reports/weekly");
    await user.click(screen.getByRole("combobox", { name: "Database" }));
    await user.click(await screen.findByRole("option", { name: "warehouse" }));
    await user.type(screen.getByLabelText("SQL"), "SELECT 1");

    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        data: { name: "Weekly rollup", path: "reports/weekly", method: "GET", database_id: "db-1", sql: "SELECT 1" },
      });
    });
    expect(await screen.findByText("endpoint detail pane")).toBeInTheDocument();
  });

  it("row menu shows Disable for a live endpoint and calls useDataApiDisableEndpoint", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "Actions" }));
    await user.click(await screen.findByText("Disable"));

    await waitFor(() => {
      expect(disableMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-1" });
    });
  });

  it("row menu shows Enable, not Disable, for a disabled endpoint", async () => {
    const disabledRow = makeEndpoint({ id: "ep-3", name: "Paused feed", state: "disabled", live_version_id: "v-2" });
    mockList({ data: { status: 200, data: [disabledRow] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Paused feed");

    await user.click(screen.getByRole("button", { name: "Actions" }));
    expect(await screen.findByText("Enable")).toBeInTheDocument();
    expect(screen.queryByText("Disable")).not.toBeInTheDocument();
  });

  it("row menu Delete opens ConfirmDeleteDialog and calls useDataApiDeleteEndpoint on confirm", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    await user.click(screen.getByRole("button", { name: "Actions" }));
    await user.click(await screen.findByText("Delete"));

    const input = await screen.findByPlaceholderText("delete");
    await user.type(input, "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-2" });
    });
  });

  it("Download OpenAPI dialog downloads the workspace-wide document with no endpoint_id", async () => {
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /download openapi/i }));
    await screen.findByRole("heading", { name: "Download OpenAPI" });

    await user.click(screen.getByRole("combobox", { name: "Service account" }));
    await user.click(await screen.findByRole("option", { name: "reporting-bot" }));
    await user.click(screen.getByRole("button", { name: /^download$/i }));

    await waitFor(() => {
      expect(api.dataApiGetOpenapi).toHaveBeenCalledWith("eng", { service_account_id: "sa-1", endpoint_id: undefined });
    });
  });

  it("Download OpenAPI dialog renders a permission message on 403, not a dead button", async () => {
    vi.mocked(api.dataApiGetOpenapi).mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /download openapi/i }));
    await user.click(screen.getByRole("combobox", { name: "Service account" }));
    await user.click(await screen.findByRole("option", { name: "reporting-bot" }));
    await user.click(screen.getByRole("button", { name: /^download$/i }));

    expect(await screen.findByText(/data_api_endpoint:manage permission/i)).toBeInTheDocument();
    // The button stays enabled/clickable — this is not a dead end.
    expect(screen.getByRole("button", { name: /^download$/i })).toBeEnabled();
  });

  it("Download OpenAPI dialog surfaces the 400 remedy verbatim (?endpoint_id=)", async () => {
    vi.mocked(api.dataApiGetOpenapi).mockRejectedValueOnce(
      new ApiError(400, "More than 200 live endpoints; download them one at a time with ?endpoint_id="),
    );
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /download openapi/i }));
    await user.click(screen.getByRole("combobox", { name: "Service account" }));
    await user.click(await screen.findByRole("option", { name: "reporting-bot" }));
    await user.click(screen.getByRole("button", { name: /^download$/i }));

    expect(await screen.findByText(/\?endpoint_id=/)).toBeInTheDocument();
  });
});
