import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
// `ListToolbarButton` (which `BulkToggle` renders) is a Radix `Tooltip`, and a
// bare `Tooltip` throws without a provider above it. Production mounts one once
// in `AppShell`; tests supply their own — same as `Charts.test.tsx`.
import { TooltipProvider } from "@/components/ui/tooltip";

// There is no msw in this repo — tests mock the API module directly (see
// AuditLog.test.tsx). Endpoints.tsx also reads folders (grouping) and
// databases (the Engine column) unconditionally, and the download dialog
// always mounts and reads service accounts even while closed — all three
// need a mock or they fire a real, unhandled `fetch` in jsdom.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/generated/folders/folders");
vi.mock("@/api/databases");
// The bulk-delete run REPORTS what it did; "Deleted 0 endpoints" over a run
// that deleted nothing is a defect only a toast assertion can see.
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { toast } from "sonner";
import * as api from "@/api/generated/data-api/data-api";
import * as foldersApi from "@/api/generated/folders/folders";
import * as databasesApi from "@/api/databases";
import { ApiError } from "@/api/client";
import type { DataApiEndpointDto } from "@/api/generated/model";
import Endpoints from "@/routes/explore/dataapi/Endpoints";

function mockList(over: Partial<{ data: unknown; isError: boolean; isLoading: boolean; error: unknown }> = {}) {
  vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
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
const DB_STARROCKS = { id: "db-2", name: "speedy", engine: "starrocks", host: "h", port: 2, options: {}, has_secret: true, status: "ok", delegated: false };

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
  // A FACTORY, not a stored element: React bails out of re-rendering a
  // referentially identical element, so `rerender(sameElement)` would not
  // re-run `Endpoints` and a test could never observe the list changing
  // underneath an open dialog.
  const tree = () => (
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints" element={<Endpoints />} />
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<p>endpoint detail pane</p>} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>
  );
  const utils = render(tree());
  return { ...utils, qc, rerenderTree: () => utils.rerender(tree()) };
}

beforeEach(() => {
  vi.clearAllMocks();
  // `useCollapsed` persists per-folder collapse state in localStorage; without
  // this, a test that collapses a folder leaks that into every later test.
  localStorage.clear();

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

  // Automocking the generated module turns the query-key builders into
  // `vi.fn()`s returning `undefined` — give them argument-sensitive fakes so
  // "the right list was invalidated" is distinguishable from
  // "invalidateQueries was called with nothing".
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws) => ["dataApiListEndpoints", ws] as unknown as ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );
  vi.mocked(api.getDataApiGetEndpointQueryKey).mockImplementation(
    (ws, id) => ["dataApiGetEndpoint", ws, id] as unknown as ReturnType<typeof api.getDataApiGetEndpointQueryKey>,
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

  it("renders name, method+path, state badge, engine and last-edited for a row", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    renderEndpoints();
    expect(await screen.findByText("Daily report")).toBeInTheDocument();
    expect(screen.getByText("GET")).toBeInTheDocument();
    expect(screen.getByText("/reports/daily")).toBeInTheDocument();
    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(screen.getByText("Trino")).toBeInTheDocument(); // resolved from database_id via useDatabases
    // There is no separate Version column — see the mutation-checked test below
    // for why that information lives in the State badge instead.
    expect(screen.queryByText("Version")).not.toBeInTheDocument();
  });

  it("shows the real error message from the failed query, not just a generic string", async () => {
    mockList({ isError: true, error: { message: "workspace not found" } });
    renderEndpoints();
    expect(await screen.findByText("workspace not found")).toBeInTheDocument();
  });

  // Reviewer finding: `EndpointStore::disable` (lifecycle.rs) has no state
  // guard and never touches `live_version_id`, so a draft can be disabled
  // directly. A bare "Disabled" badge would read as "currently serving" for
  // an endpoint that never served anything — this is the test that pins the
  // fix and the one Step 7's mutation check below targets.
  it("distinguishes a disabled-but-was-live endpoint from a disabled-and-never-published one", async () => {
    const wasLive = makeEndpoint({ id: "ep-5", name: "Paused feed", state: "disabled", live_version_id: "v-2" });
    const neverPublished = makeEndpoint({ id: "ep-6", name: "Stillborn draft", state: "disabled", live_version_id: null });
    mockList({ data: { status: 200, data: [wasLive, neverPublished] } });
    renderEndpoints();
    expect(await screen.findByText("Disabled · was live")).toBeInTheDocument();
    expect(screen.getByText("Disabled · never published")).toBeInTheDocument();
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

    await user.click(screen.getByRole("button", { name: "Actions for Daily report" }));
    await user.click(await screen.findByText("Disable"));

    await waitFor(() => {
      expect(disableMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-1" });
    });
  });

  it("row menu shows Enable, not Disable, for a disabled endpoint, and it is clickable when there's a version to resume", async () => {
    const disabledRow = makeEndpoint({ id: "ep-3", name: "Paused feed", state: "disabled", live_version_id: "v-2" });
    mockList({ data: { status: 200, data: [disabledRow] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Paused feed");

    await user.click(screen.getByRole("button", { name: "Actions for Paused feed" }));
    const enableItem = await screen.findByText("Enable");
    expect(enableItem).toBeInTheDocument();
    expect(screen.queryByText("Disable")).not.toBeInTheDocument();
    expect(enableItem.closest('[role="menuitem"]')).not.toHaveAttribute("data-disabled");
  });

  // `useDataApiEnableEndpoint` 409s (EndpointStore::enable, lifecycle.rs) when
  // `live_version_id` is null — nothing to resume. The menu item must say so
  // up front rather than let the user find out through a failed-mutation toast.
  it("row menu disables Enable with an explanatory title for a never-published endpoint", async () => {
    const neverPublished = makeEndpoint({ id: "ep-7", name: "Stillborn draft", state: "disabled", live_version_id: null });
    mockList({ data: { status: 200, data: [neverPublished] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Stillborn draft");

    await user.click(screen.getByRole("button", { name: "Actions for Stillborn draft" }));
    const enableItem = await screen.findByText("Enable");
    const menuitem = enableItem.closest('[role="menuitem"]')!;
    expect(menuitem).toHaveAttribute("data-disabled");
    expect(menuitem).toHaveAttribute("title", expect.stringContaining("never been published"));

    await user.click(enableItem);
    expect(enableMutateAsync).not.toHaveBeenCalled();
  });

  it("row menu Delete opens ConfirmDeleteDialog and calls useDataApiDeleteEndpoint on confirm", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    await user.click(screen.getByRole("button", { name: "Actions for Ad hoc query" }));
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

/**
 * Cache invalidation (final-review finding, Important #1).
 *
 * `queryClient.ts` sets `staleTime: 30000` and the orval-generated mutation
 * hooks carry no `onSuccess` — there is not one `invalidate` in the generated
 * file. So a mutation wired straight to a generated hook toasts success over a
 * list that goes on showing the OLD state for the next 30 seconds: Disable
 * leaves the badge reading Live, Delete leaves the row in place, Create
 * navigates away and Back shows a list without the new endpoint.
 *
 * These tests drive the `onSuccess` this page's mutations were constructed
 * with — the one `src/api/dataApi.ts` attaches — exactly the way
 * `EndpointDetail.test.tsx` pins Save's. Asserting on the KEY, not merely on
 * "invalidateQueries ran", is what makes them fail for a wrapper that
 * invalidates the wrong workspace or the wrong query.
 */
describe("Endpoints — cache invalidation", () => {
  const RESULT = { status: 200, data: {}, headers: new Headers() } as never;

  /** Fire the `onSuccess` the page's mutation was built with, if it has one. */
  async function fireSuccess(
    hook: { mock: { calls: unknown[][] } },
    variables: Record<string, unknown>,
  ) {
    const opts = hook.mock.calls.at(-1)?.[0] as
      | { mutation?: { onSuccess?: (...a: never[]) => unknown } }
      | undefined;
    await opts?.mutation?.onSuccess?.(
      RESULT,
      variables as never,
      undefined as never,
      undefined as never,
    );
  }

  it("Disable invalidates the endpoint list, so the badge cannot keep reading Live", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    // The disable/enable mutations are constructed by `EndpointActionsMenu`,
    // i.e. per rendered ROW — so the row has to actually be on screen, and
    // `groupByFolder` drops a row whose `folder_id` names a folder the list
    // does not contain. Without this the hook is never called and `fireSuccess`
    // silently fires nothing.
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDisableEndpoint), { workspace: "eng", id: "ep-1" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });

  it("Enable invalidates the endpoint list, so the badge cannot keep reading Disabled", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiEnableEndpoint), { workspace: "eng", id: "ep-1" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });

  it("Delete invalidates the endpoint list, so the deleted row leaves the table", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDeleteEndpoint), { workspace: "eng", id: "ep-2" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });

  it("Create invalidates the endpoint list, so navigating Back shows the new endpoint", async () => {
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiCreateEndpoint), { workspace: "eng", data: {} });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });

  // The workspace comes from the mutation VARIABLES, not from a hook argument,
  // so there is no second source of truth that can drift from the write.
  it("invalidates the list of the workspace the write actually went to", async () => {
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDeleteEndpoint), { workspace: "other-ws", id: "ep-2" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "other-ws"] }),
    );
  });
});

/**
 * Endpoints is a top-level sidebar item (its own Data API sidebar group), not
 * a section nested under a `DataApiLayout` shell — that shell was removed
 * once Data API got its own sidebar group (Endpoints/Clients/Activity), so
 * this page owns its own `h1` and `p-4`, exactly like `Charts.tsx`.
 */
describe("Endpoints — page scaffold", () => {
  it("renders its title as the page's own h1", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    renderEndpoints();
    const h1 = await screen.findByRole("heading", { level: 1, name: "Endpoints" });
    expect(h1.className).toContain("text-lg");
    expect(screen.queryByRole("heading", { level: 2, name: "Endpoints" })).not.toBeInTheDocument();
  });

  it("carries its own p-4 padding, like every other top-level Explore list page", async () => {
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const { container } = renderEndpoints();
    await screen.findByRole("heading", { level: 1, name: "Endpoints" });
    expect(container.firstElementChild?.className ?? "").toMatch(/\bp-4\b/);
  });
});

/**
 * Disable/Enable fire from a `DropdownMenuItem`, which closes on click but can
 * be reopened before the request settles. Every other mutation trigger in this
 * feature guards on `isPending`; these two did not.
 */
describe("Endpoints — in-flight guards", () => {
  it("disables the Disable item while a disable is in flight", async () => {
    vi.mocked(api.useDataApiDisableEndpoint).mockReturnValue(
      { mutateAsync: disableMutateAsync, isPending: true } as unknown as
        ReturnType<typeof api.useDataApiDisableEndpoint>,
    );
    mockList({ data: { status: 200, data: [FOLDERED_ROW] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "Actions for Daily report" }));
    const item = (await screen.findByText("Disable")).closest('[role="menuitem"]')!;
    expect(item).toHaveAttribute("data-disabled");
  });

  it("disables the Enable item while an enable is in flight, and a click cannot re-fire it", async () => {
    vi.mocked(api.useDataApiEnableEndpoint).mockReturnValue(
      { mutateAsync: enableMutateAsync, isPending: true } as unknown as
        ReturnType<typeof api.useDataApiEnableEndpoint>,
    );
    const resumable = makeEndpoint({ id: "ep-8", name: "Paused feed", state: "disabled", live_version_id: "v-9" });
    mockList({ data: { status: 200, data: [resumable] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Paused feed");

    await user.click(screen.getByRole("button", { name: "Actions for Paused feed" }));
    const label = await screen.findByText("Enable");
    expect(label.closest('[role="menuitem"]')).toHaveAttribute("data-disabled");
    // `data-[disabled]:pointer-events-none` is CSS, which jsdom does not apply
    // — the handler has to refuse on its own, same as the never-published case.
    await user.click(label);
    expect(enableMutateAsync).not.toHaveBeenCalled();
  });
});

/**
 * Final-review finding (Important #4), second surface: the download dialog's
 * account picker read the SAME query as Try-it's and collapsed `isError` into
 * the same empty case — telling the user to "create one under Clients" when
 * the request had failed, next to a Download button that could never succeed.
 */
describe("DownloadOpenApiDialog — a failed account fetch is not an empty workspace", () => {
  function mockAccounts(over: Record<string, unknown>) {
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
        ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
  }

  it("reports a failed fetch as a failure, never as 'no service accounts yet'", async () => {
    mockAccounts({ isError: true, error: new Error("workspace not found") });
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /download openapi/i }));
    await screen.findByRole("heading", { name: "Download OpenAPI" });

    expect(screen.getByText(/could not load service accounts/i)).toBeInTheDocument();
    expect(screen.getByText(/workspace not found/)).toBeInTheDocument();
    expect(screen.queryByText(/no service accounts yet/i)).not.toBeInTheDocument();
  });

  it("still shows the create-one prompt when the workspace is genuinely empty", async () => {
    mockAccounts({ data: { status: 200, data: [] } });
    const user = userEvent.setup();
    renderEndpoints();

    await user.click(screen.getByRole("button", { name: /download openapi/i }));
    await screen.findByRole("heading", { name: "Download OpenAPI" });

    expect(screen.getByText(/no service accounts yet/i)).toBeInTheDocument();
    expect(screen.queryByText(/could not load service accounts/i)).not.toBeInTheDocument();
  });
});

/**
 * Task 5: the page adopts the shared Explore list primitives (`ListPageShell`,
 * `ListStates`, `EmptyState`, `BulkToggle`/`BulkBar`, `ListToolbarButton`) and
 * gains the two capabilities it lacked that need no backend — facet filters and
 * a row-actions menu of its own.
 */
describe("Endpoints — facets, row menu and bulk mode", () => {
  it("filters by state through a facet, not only by clicking a summary chip", async () => {
    // The state filter exists today but is reachable only by clicking a chip in
    // the SummaryStrip, which does not read as a filter control.
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "live one", state: "live", live_version_id: "v-1" }),
          makeEndpoint({ id: "e2", name: "draft one", state: "draft" }),
        ],
      },
    });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("live one");

    await user.click(screen.getByRole("combobox", { name: "State" }));
    await user.click(await screen.findByRole("option", { name: "draft" }));

    expect(screen.queryByText("live one")).not.toBeInTheDocument();
    expect(screen.getByText("draft one")).toBeInTheDocument();
  });

  // The facet and the SummaryStrip chip are two controls over ONE piece of
  // state. If they were separate, filtering to `draft` by chip would leave the
  // facet reading "All" — a page showing two filters that disagree.
  it("the summary chip and the facet drive the same state filter", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "live one", state: "live", live_version_id: "v-1" }),
          makeEndpoint({ id: "e2", name: "draft one", state: "draft" }),
        ],
      },
    });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("live one");

    // Filter by chip… (`/draft$/`, not `/draft/`: the row menus are named after
    // their row, and one of these rows is called "draft one".)
    await user.click(screen.getByRole("button", { name: /draft$/ }));
    // …and the facet must have moved with it.
    expect(screen.getByRole("combobox", { name: "State" })).toHaveTextContent("draft");

    // Clear it through the facet, and the chip must let go too.
    await user.click(screen.getByRole("combobox", { name: "State" }));
    await user.click(await screen.findByRole("option", { name: "All" }));
    expect(screen.getByRole("button", { name: /draft$/ })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("live one")).toBeInTheDocument();
  });

  it("filters by engine through a facet", async () => {
    vi.mocked(databasesApi.useDatabases).mockReturnValue(
      { data: [DB_TRINO, DB_STARROCKS], isLoading: false, isError: false } as unknown as
        ReturnType<typeof databasesApi.useDatabases>,
    );
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "on trino", database_id: "db-1" }),
          makeEndpoint({ id: "e2", name: "on starrocks", database_id: "db-2" }),
        ],
      },
    });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("on trino");

    await user.click(screen.getByRole("combobox", { name: "Engine" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));

    expect(screen.queryByText("on trino")).not.toBeInTheDocument();
    expect(screen.getByText("on starrocks")).toBeInTheDocument();
  });

  it("shows the error, not the empty state, when the list fails to load", async () => {
    mockList({ isError: true, error: new Error("nope") });
    renderEndpoints();
    expect(await screen.findByText("nope")).toBeInTheDocument();
    expect(screen.queryByText(/no endpoints yet/i)).not.toBeInTheDocument();
  });

  it("puts row actions in a menu rather than loose in the row", async () => {
    mockList({ data: { status: 200, data: [makeEndpoint({ id: "e1", name: "one", state: "live", live_version_id: "v-1" })] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: /actions/i }));
    expect(await screen.findByRole("menuitem", { name: /open/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /disable/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /delete/i })).toBeInTheDocument();
  });

  it("offers a bulk action only once rows are selected", async () => {
    mockList({ data: { status: 200, data: [makeEndpoint({ id: "e1", name: "one", state: "draft" })] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // Bulk mode is on but nothing is chosen: the action strip must not be there
    // at all, or the table jumps down the moment the mode is entered.
    expect(screen.queryByRole("button", { name: /delete selected/i })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: /select one/i }));
    expect(screen.getByRole("button", { name: /delete selected/i })).toBeInTheDocument();
  });

  it("bulk Delete selected deletes every selected row after confirmation", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "one", state: "draft" }),
          makeEndpoint({ id: "e2", name: "two", state: "draft" }),
        ],
      },
    });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: /delete selected/i }));

    await user.type(await screen.findByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e1" });
    });
    expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e2" });
  });
});

/**
 * Fix round 1: `useBulkSelect` was fed the pre-grouping `rows`, so "Select all"
 * selected rows that never reached the screen. Before this page had a bulk
 * mode those rows were merely invisible; with one, they became deletable.
 *
 * Both tests below are about the DANGEROUS case on purpose. A bulk test using
 * only rows in a known, expanded folder passes with the bug fully present.
 */
describe("Endpoints — bulk select only ever reaches rows that are on screen", () => {
  it("does not select or delete a row groupByFolder dropped for an orphan folder_id", async () => {
    // `f-missing` is not in the folders list — the row a concurrent folder
    // delete left behind. `groupByFolder` emits no bucket for it, so the row
    // is not rendered at all.
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e-orphan", name: "orphan row", folder_id: "f-missing" }),
          makeEndpoint({ id: "e-shown", name: "shown row", folder_id: null }),
        ],
      },
    });
    mockFolders([]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("shown row");
    expect(screen.queryByText("orphan row")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));

    // The count must describe what is in front of the user, not the filtered set.
    expect(screen.getByText("1 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /delete selected/i }));
    await user.type(await screen.findByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    expect(deleteMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "e-orphan" });
    expect(deleteMutateAsync).toHaveBeenCalledTimes(1);
  });

  it("does not select a collapsed folder's rows, which are grouped but not on screen", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e-hidden", name: "hidden row", folder_id: "f-1" }),
          makeEndpoint({ id: "e-shown", name: "shown row", folder_id: null }),
        ],
      },
    });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("hidden row");

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("hidden row")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));

    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "Select hidden row" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /delete selected/i }));
    await user.type(await screen.findByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    expect(deleteMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "e-hidden" });
  });
});

/**
 * Final-review finding (Important #1). The bulk fix has TWO independent halves:
 * the call site narrowing `useBulkSelect`'s input to `visibleRows` (which
 * constrains `toggleAll`), and `selectedIds` re-filtering through `rows` on
 * every render. Only the first was covered — both tests above HIDE first and
 * SELECT second, so `toggleAll` never had a chance to write a hidden id and the
 * re-filter was never the thing under test. Replacing `selectedIds` with
 * `Array.from(selected)` typechecked and left every test green.
 *
 * These reverse the order. The failure they pin is the one this page cannot
 * take back: its bulk action is a hard DELETE, not a move to trash.
 */
describe("Endpoints — a row selected BEFORE it is hidden", () => {
  const HIDDEN = makeEndpoint({ id: "e-hidden", name: "hidden row", folder_id: "f-1" });
  const SHOWN = makeEndpoint({ id: "e-shown", name: "shown row", folder_id: null });

  it("stops counting a selected row the moment its folder collapses", async () => {
    mockList({ data: { status: 200, data: [HIDDEN, SHOWN] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("hidden row");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first — the row is on screen and legitimately selectable.
    await user.click(screen.getByRole("checkbox", { name: "Select hidden row" }));
    expect(screen.getByText("1 selected")).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved at all here: the id is already
    // in `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("hidden row")).not.toBeInTheDocument();

    expect(screen.queryByText("1 selected")).not.toBeInTheDocument();
    // The strip renders nothing at zero, so the destructive button goes with it
    // — there is no way left to act on a row that is not on screen.
    expect(screen.queryByRole("button", { name: /delete selected/i })).not.toBeInTheDocument();
  });

  it("deletes only the row still on screen when a selected row is hidden behind it", async () => {
    mockList({ data: { status: 200, data: [HIDDEN, SHOWN] } });
    mockFolders([{ id: "f-1", name: "Reporting" }]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("hidden row");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select hidden row" }));
    await user.click(screen.getByRole("checkbox", { name: "Select shown row" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByText("1 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /delete selected/i }));
    await user.type(await screen.findByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    // The one the user can no longer see must survive.
    expect(deleteMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "e-hidden" });
    expect(deleteMutateAsync).toHaveBeenCalledTimes(1);
  });
});

/**
 * A list whose rows change under an open dialog, and a mode whose only OFF
 * switch can unmount (final review, Minor #7).
 */
describe("Endpoints — the list changing underneath the page", () => {
  /** A list hook whose data is read fresh on every render, not frozen at mock time. */
  function mockListDynamic(get: () => DataApiEndpointDto[]) {
    vi.mocked(api.useDataApiListEndpoints).mockImplementation(
      () => ({ data: { status: 200, data: get() }, isLoading: false, isError: false, error: null }) as unknown as
        ReturnType<typeof api.useDataApiListEndpoints>,
    );
  }

  it("does not report a bulk delete it never ran when the selection empties behind the dialog", async () => {
    let rows: DataApiEndpointDto[] = [makeEndpoint({ id: "e1", name: "one", state: "draft" })];
    mockListDynamic(() => rows);
    const user = userEvent.setup();
    const { rerenderTree } = renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select one" }));
    await user.click(screen.getByRole("button", { name: /delete selected/i }));
    await screen.findByPlaceholderText("delete");

    // A refetch lands while the confirmation sits open and the row is gone.
    rows = [];
    rerenderTree();

    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    expect(deleteMutateAsync).not.toHaveBeenCalled();
    // …and above all it must not claim it did something. "Deleted 0 endpoints"
    // is a success toast over a run that deleted nothing.
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("leaves bulk mode behind when the last endpoint goes, so the checkbox column does not return with the next one", async () => {
    let rows: DataApiEndpointDto[] = [makeEndpoint({ id: "e1", name: "one", state: "draft" })];
    mockListDynamic(() => rows);
    const user = userEvent.setup();
    const { rerenderTree } = renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    expect(screen.getByRole("checkbox", { name: "Select all" })).toBeInTheDocument();

    // Every row is deleted. The toggle — bulk mode's only OFF switch — unmounts
    // with the table, so the mode has to let go of itself here.
    rows = [];
    rerenderTree();
    await screen.findByText(/no endpoints yet/i);

    // A new endpoint is created.
    rows = [makeEndpoint({ id: "e2", name: "two", state: "draft" })];
    rerenderTree();
    await screen.findByText("two");

    expect(screen.queryByRole("checkbox", { name: "Select all" })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "Select two" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Bulk select" })).toHaveAttribute("aria-pressed", "false");
  });
});

/**
 * Minor #6: the summary strip used to mix one filtered count (`rows.length`)
 * with three unfiltered ones, so filtering by Engine produced four numbers on
 * one line that did not add up.
 */
describe("Endpoints — the summary strip is internally consistent", () => {
  it("keeps all four numbers on the same (unfiltered) set while a facet is applied", async () => {
    vi.mocked(databasesApi.useDatabases).mockReturnValue(
      { data: [DB_TRINO, DB_STARROCKS], isLoading: false, isError: false } as unknown as
        ReturnType<typeof databasesApi.useDatabases>,
    );
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "on trino", database_id: "db-1", state: "live", live_version_id: "v-1" }),
          makeEndpoint({ id: "e2", name: "on starrocks", database_id: "db-2", state: "draft" }),
          makeEndpoint({ id: "e3", name: "also starrocks", database_id: "db-2", state: "draft" }),
        ],
      },
    });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("on trino");

    await user.click(screen.getByRole("combobox", { name: "Engine" }));
    await user.click(await screen.findByRole("option", { name: "StarRocks" }));

    // The table is filtered…
    expect(screen.queryByText("on trino")).not.toBeInTheDocument();
    expect(screen.getByText("on starrocks")).toBeInTheDocument();

    // …and the strip still describes ONE set: 1 + 2 + 0 = 3, the total it
    // states. Before the fix this read "2 endpoints · 1 live · 2 draft · 0
    // disabled" — a leading number that matched none of the three beside it.
    expect(screen.getByText("endpoints total").textContent).toBe("3endpoints total");
    expect(screen.getByText("live").textContent).toBe("1live");
    expect(screen.getByText("draft").textContent).toBe("2draft");
    expect(screen.getByText("disabled").textContent).toBe("0disabled");
  });
});

/**
 * Minor #4: `ServiceAccountActionsMenu` names its trigger after its row and
 * this one did not, so a 30-row Endpoints list gave a screen-reader user 30
 * controls all called "Actions".
 */
describe("Endpoints — row menus are named after their row", () => {
  it("gives every row's actions trigger its own accessible name", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ id: "e1", name: "one", state: "draft" }),
          makeEndpoint({ id: "e2", name: "two", state: "draft" }),
        ],
      },
    });
    renderEndpoints();
    await screen.findByText("one");

    // `getByRole` throws on more than one match, so this fails outright if both
    // rows fall back to the bare default.
    expect(screen.getByRole("button", { name: "Actions for one" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Actions for two" })).toBeInTheDocument();
  });
});
