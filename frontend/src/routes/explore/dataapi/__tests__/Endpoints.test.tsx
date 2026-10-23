import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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
import type { DataApiEndpointListItemDto } from "@/api/generated/model";
// The exact string `RelativeTime` puts in its `title` — imported rather than
// hardcoded so the assertion does not silently become timezone-dependent.
import { fullTimestamp } from "@/lib/relativeTime";
// The folderless bucket's collapse key, imported rather than retyped: the
// stranded-collapse test below seeds localStorage with exactly what
// `useCollapsed` will look for, and a hardcoded "__uncat__" would keep
// passing against a page that had stopped using this key at all.
import { UNCATEGORIZED_KEY } from "@/components/explore/folders";
import Endpoints, { folderCollapseKey } from "@/routes/explore/dataapi/Endpoints";
// The shipped glyph map, so a forbidden glyph is named by its TIER and not by a
// class string that lucide is free to rename out from under the assertion.
import { TIER_ICON } from "@/routes/explore/dataapi/endpointState";

function mockList(over: Partial<{ data: unknown; isError: boolean; isLoading: boolean; error: unknown }> = {}) {
  vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
      ReturnType<typeof api.useDataApiListEndpoints>,
  );
}

/**
 * The list mock, but ANSWERING THE `trashed` PARAM — the normal view and the
 * trash return different rows.
 *
 * `mockList` above returns one payload whatever it is asked for, which cannot
 * distinguish a Trash toggle that re-queries the API from one that only
 * repaints the same rows: both render the same table. This is the fixture the
 * trash tests use for that reason.
 */
function mockListByView(normal: DataApiEndpointListItemDto[], trashed: DataApiEndpointListItemDto[]) {
  vi.mocked(api.useDataApiListEndpoints).mockImplementation(((_ws: string, params?: { trashed?: boolean }) => ({
    data: { status: 200, data: params?.trashed ? trashed : normal },
    isLoading: false,
    isError: false,
    error: null,
  })) as unknown as typeof api.useDataApiListEndpoints);
}

function mockFolders(folders: { id: string; name: string }[] = []) {
  vi.mocked(foldersApi.useFoldersFoldersList).mockReturnValue(
    { data: { status: 200, data: folders }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof foldersApi.useFoldersFoldersList>,
  );
}

const DB_TRINO = { id: "db-1", name: "warehouse", engine: "trino", host: "h", port: 1, options: {}, has_secret: true, status: "ok", delegated: false };
const DB_STARROCKS = { id: "db-2", name: "speedy", engine: "starrocks", host: "h", port: 2, options: {}, has_secret: true, status: "ok", delegated: false };

function makeEndpoint(overrides: Partial<DataApiEndpointListItemDto> = {}): DataApiEndpointListItemDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    statements: [{ name: "main", sql: "select 1" }],
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
    // The list DTO carries the CALLER's own flag (`DataApiEndpointListItemDto`),
    // so a fixture without it would render every star in a state the server
    // never sent. Default false; the bookmark tests override it per row.
    bookmarked: false,
    ...overrides,
  } as unknown as DataApiEndpointListItemDto;
}

const FOLDERED_ROW = makeEndpoint({
  id: "ep-1", name: "Daily report", folder_id: "f-1", state: "live", live_version_id: "v-1",
});
const LOOSE_ROW = makeEndpoint({
  id: "ep-2", name: "Ad hoc query", folder_id: null, state: "draft",
});

const SERVICE_ACCOUNT = { id: "sa-1", name: "reporting-bot", description: "", email: "sa@egret.local", created_at: "2026-08-01T00:00:00Z" };

const deleteMutateAsync = vi.fn();
const bookmarkMutateAsync = vi.fn();
const unbookmarkMutateAsync = vi.fn();
const disableMutateAsync = vi.fn();
const enableMutateAsync = vi.fn();
const updateMutateAsync = vi.fn();
const trashMutateAsync = vi.fn();
const restoreMutateAsync = vi.fn();

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

  vi.mocked(api.useDataApiDeleteEndpoint).mockReturnValue(
    { mutateAsync: deleteMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiDeleteEndpoint>,
  );
  vi.mocked(api.useDataApiBookmarkEndpoint).mockReturnValue(
    { mutateAsync: bookmarkMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiBookmarkEndpoint>,
  );
  vi.mocked(api.useDataApiUnbookmarkEndpoint).mockReturnValue(
    { mutateAsync: unbookmarkMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiUnbookmarkEndpoint>,
  );
  vi.mocked(api.useDataApiDisableEndpoint).mockReturnValue(
    { mutateAsync: disableMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiDisableEndpoint>,
  );
  vi.mocked(api.useDataApiEnableEndpoint).mockReturnValue(
    { mutateAsync: enableMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiEnableEndpoint>,
  );
  // Moving a row between folders is a PATCH on the endpoint itself — the same
  // `UpdateEndpointInput` the detail route saves through — so the folder tests
  // assert on THIS mock's payload.
  vi.mocked(api.useDataApiUpdateEndpoint).mockReturnValue(
    { mutateAsync: updateMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiUpdateEndpoint>,
  );
  vi.mocked(api.useDataApiTrashEndpoint).mockReturnValue(
    { mutateAsync: trashMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiTrashEndpoint>,
  );
  vi.mocked(api.useDataApiRestoreEndpoint).mockReturnValue(
    { mutateAsync: restoreMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiRestoreEndpoint>,
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
  // Params-aware, exactly like the real builder (which appends the params
  // object when there is one). That is what lets the trash tests below assert
  // the invalidated key is the bare PREFIX — a wrapper that invalidated
  // `[..., {trashed: true}]` would refresh only the view the user is already
  // looking at, and the assertion has to be able to see the difference.
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws, params) => (params ? ["dataApiListEndpoints", ws, params] : ["dataApiListEndpoints", ws]) as unknown as
      ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );
  vi.mocked(api.getDataApiGetEndpointQueryKey).mockImplementation(
    (ws, id) => ["dataApiGetEndpoint", ws, id] as unknown as ReturnType<typeof api.getDataApiGetEndpointQueryKey>,
  );

  deleteMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  bookmarkMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  unbookmarkMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  disableMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  enableMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  updateMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  trashMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });
  restoreMutateAsync.mockResolvedValue({ status: 200, data: {}, headers: new Headers() });

  // `useFolderMutations` (the page's New-folder path) runs a REAL `useMutation`
  // over this automocked fetch fn; without a resolved value it returns
  // `undefined` and `.data` throws inside the mutation.
  vi.mocked(foldersApi.foldersFoldersCreate).mockResolvedValue(
    { status: 201, data: { id: "f-new", name: "New folder" }, headers: new Headers() } as unknown as
      Awaited<ReturnType<typeof foldersApi.foldersFoldersCreate>>,
  );
  vi.mocked(foldersApi.getFoldersFoldersListQueryKey).mockImplementation(
    (ws, kind) => ["foldersFoldersList", ws, kind] as unknown as
      ReturnType<typeof foldersApi.getFoldersFoldersListQueryKey>,
  );

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

  // KI-231: the server refusing the list is the shared refusal, not a red box
  // with "forbidden" under a title bar still offering + Endpoint.
  it("renders the shared refusal, without the page's actions, when the server refuses the list", async () => {
    mockList({ isError: true, error: new ApiError(403, "forbidden") });
    renderEndpoints();
    expect(
      await screen.findByRole("heading", { level: 1, name: "You don't have access to this page." }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Viewing endpoints needs the data_api_endpoint:read permission in this workspace."),
    ).toBeInTheDocument();
    expect(screen.queryByText("forbidden")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "+ Endpoint" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Download OpenAPI" })).not.toBeInTheDocument();
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

  /**
   * THE ROW BADGE IS THE EDITOR'S PILL, AND IT IS NOT ALLOWED TO GUESS.
   *
   * The list kept its own tone map — `live: "bg-success/15 text-success"`,
   * measured at 3.72:1 in Chromium against WCAG 2.2 1.4.3's 4.5:1 for this 12px
   * text — while the header pill one click away had already been moved to
   * `--foreground` ink plus a per-tier glyph. Same endpoint, two paintings.
   *
   * Moving the list onto the shared component has one trap, and these tests
   * exist for it. `endpointStateTier` defaults `served` to `"unknown"`, and for
   * a LIVE endpoint `"unknown"` is a tier of its own: grey `bg-muted` with a
   * `CircleHelp`. The list does not fetch versions, so a naive rewiring paints
   * every live row as a shrug — and passing `"in-step"` to get the green back
   * is a claim the list has no evidence for. `endpointStateOnlyTier` is the
   * third answer, and its return type is what makes the wrong two unreachable.
   */
  const ROW_TIERS = [
    ["live", "bg-success/15"],
    ["draft", "bg-muted"],
    ["disabled", "bg-destructive/15"],
  ] as const;

  /**
   * The lucide name classes an icon paints, sorted and joined into one identity.
   *
   * Taken from the ICON ITSELF rather than typed as a literal, because the
   * literal is not checkable: `CircleHelp` is an alias of `CircleQuestionMark`
   * and paints `lucide-circle-question-mark`, `CircleSlash2` paints TWO name
   * classes, and a `not.toBe("lucide-circle-help")` written from the import name
   * is true under every possible render. That assertion was written here first
   * and it survived the mutant it existed to kill.
   */
  function glyphOf(Icon: (typeof TIER_ICON)[keyof typeof TIER_ICON]): string {
    const { container, unmount } = render(<Icon />);
    const names = lucideNames(container.querySelector("svg")!);
    unmount();
    return names;
  }

  function lucideNames(svg: Element): string {
    const named = svg
      .getAttribute("class")!
      .split(/\s+/)
      .filter((c) => c.startsWith("lucide-") && c !== "lucide")
      .sort()
      .join(" ");
    // An empty identity would make every comparison below pass by being
    // uniformly nothing, so it is rejected before anything is compared.
    expect(named).not.toBe("");
    return named;
  }

  /** The state badge of the one row on screen, and the glyph inside it. */
  function stateBadge() {
    const icon = screen.getByTestId("endpoint-state-icon");
    return { className: icon.closest('[data-slot="badge"]')!.className, glyph: lucideNames(icon) };
  }

  it("a live row is painted LIVE, not `unknown` — the list is not asking about divergence", async () => {
    // The mutation this pins: `endpointStateTier(endpoint)` in place of
    // `endpointStateOnlyTier(endpoint)` inside `EndpointRowStateBadge`. It
    // compiles (`served` is optional) and it type-checks, and it turns every
    // live row grey with a question mark. Confirmed by making that exact edit:
    // `tsc` stayed at 0 errors and this test failed on both assertions.
    mockList({ data: { status: 200, data: [makeEndpoint({ state: "live", live_version_id: "v-1" })] } });
    renderEndpoints();
    await screen.findByText("Live");
    const badge = stateBadge();
    expect(badge.glyph).toBe(glyphOf(TIER_ICON.live));
    expect(badge.className).toContain("bg-success/15");
  });

  it("no row can reach the `diverged` or `unknown` tier, in either channel", async () => {
    // A row that has not asked whether the draft is ahead must not answer in
    // either direction, so neither the amber `bg-highlight/15` wash nor the two
    // glyphs that mean "behind" and "cannot tell" may appear on any state the
    // list can render. The two glyphs are read off `TIER_ICON`, so a lucide
    // rename cannot quietly make this vacuous.
    const forbidden = [glyphOf(TIER_ICON.unknown), glyphOf(TIER_ICON.diverged)];
    for (const [state] of ROW_TIERS) {
      mockList({ data: { status: 200, data: [makeEndpoint({ state, live_version_id: "v-1" })] } });
      const { unmount } = renderEndpoints();
      await screen.findByTestId("endpoint-state-icon");
      const badge = stateBadge();
      expect(forbidden, `${state}: ${badge.glyph}`).not.toContain(badge.glyph);
      expect(badge.className, state).not.toContain("bg-highlight");
      unmount();
    }
  });

  it("every row state has its own tone AND its own glyph, so state is not carried by colour alone", async () => {
    const seen: string[] = [];
    for (const [state, fill] of ROW_TIERS) {
      mockList({ data: { status: 200, data: [makeEndpoint({ state, live_version_id: "v-1" })] } });
      const { unmount } = renderEndpoints();
      await screen.findByTestId("endpoint-state-icon");
      const badge = stateBadge();
      // The tier is named by the TEST and the glyph is derived from the shipped
      // map, so this is not a tautology: the component still has to route the
      // state to the tier the test names.
      expect(badge.glyph, state).toBe(glyphOf(TIER_ICON[state]));
      expect(badge.className, state).toContain(fill);
      seen.push(badge.glyph);
      unmount();
    }
    // Against the state NAMES rather than `seen.length`, which would read the
    // table on both sides of the equality and shrink silently with it.
    expect(ROW_TIERS.map(([state]) => state)).toEqual(["live", "draft", "disabled"]);
    expect(new Set(seen).size).toBe(3);
  });

  it("the row badge's ink is --foreground on every state, never a tinted ink", async () => {
    // Measured in Chromium by painting the composited colour into a 1x1 canvas
    // and reading the pixel back (`tests/e2e/dataapi-badge-ink.spec.ts`):
    // `text-success` on `bg-success/15` is 3.72:1 light, `text-destructive` on
    // `bg-destructive/15` is 3.68:1, and `--foreground` on the same two fills
    // is 14.76:1 and 13.81:1. Nothing paints in jsdom and nothing here resolves
    // `oklch()`, so this is the WIRING guard on that decision — that each row
    // is handed `text-foreground` and no hued ink — and the browser spec is the
    // measurement.
    for (const [state] of ROW_TIERS) {
      mockList({ data: { status: 200, data: [makeEndpoint({ state, live_version_id: "v-1" })] } });
      const { unmount } = renderEndpoints();
      await screen.findByTestId("endpoint-state-icon");
      const { className } = stateBadge();
      expect(className, state).toContain("text-foreground");
      for (const ink of ["text-success", "text-destructive", "text-highlight", "text-muted-foreground"]) {
        expect(className, `${state}: ${ink}`).not.toContain(ink);
      }
      unmount();
    }
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

  /**
   * Creating an endpoint used to be a dialog this page owned; it is now the
   * `endpoints/new` PAGE (`EndpointCreate`), which owns the fields and the
   * mutation. What is left for the list to get right is that both of its
   * entry points are real LINKS to that URL — a `<button>` that pushed the
   * route imperatively would look identical on screen but could not be
   * middle-clicked, opened in a new tab, or copied.
   *
   * Asserting the `href` rather than clicking is deliberate: this harness maps
   * `endpoints/:id` to a marker, so a click would "pass" even for a link
   * pointing at some other id.
   */
  it("the + Endpoint action is a link to the create page, not a dialog trigger", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    renderEndpoints();
    await screen.findByText("Ad hoc query");
    expect(screen.getByRole("link", { name: /\+ endpoint/i })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/endpoints/new",
    );
    expect(screen.queryByRole("button", { name: /\+ endpoint/i })).not.toBeInTheDocument();
  });

  it("the empty-state CTA links to the create page too", async () => {
    mockList({ data: { status: 200, data: [] } });
    renderEndpoints();
    expect(
      await screen.findByRole("link", { name: /create your first endpoint/i }),
    ).toHaveAttribute("href", "/explore/eng/data-api/endpoints/new");
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

  // Permanent delete is reachable only from the TRASH, so the only route to
  // this dialog now runs through the Trash view. `LOOSE_ROW`'s id is still what
  // has to reach the mutation — the dialog is opened from a row, and a dialog
  // that deleted the wrong one would look identical here.
  it("Delete permanently opens ConfirmDeleteDialog and calls useDataApiDeleteEndpoint on confirm", async () => {
    mockListByView([], [LOOSE_ROW]);
    const user = userEvent.setup();
    renderEndpoints();
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Ad hoc query");

    await user.click(screen.getByRole("button", { name: "Actions for Ad hoc query" }));
    await user.click(await screen.findByText("Delete permanently"));

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
    // i.e. per rendered ROW — so the row has to actually be on screen, and this
    // row sits in `f-1`. Keep the folder mocked: without a rendered row the
    // hook is never called and `fireSuccess` silently fires nothing.
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

  // Create's invalidation moved with the mutation: this page no longer builds
  // it, `EndpointCreate` does. The same assertion lives in
  // `EndpointCreate.invalidation.test.tsx`.

  // `bookmarked` only exists on the LIST row, so the list is the one query a
  // star can go stale in — and with `staleTime: 30000` an un-invalidated star
  // springs back to its old fill on the next render.
  it("Bookmark invalidates the endpoint list, so the star cannot spring back", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiBookmarkEndpoint), { workspace: "eng", id: "ep-2" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });

  it("Unbookmark invalidates the endpoint list too", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    const { qc } = renderEndpoints();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiUnbookmarkEndpoint), { workspace: "eng", id: "ep-2" });
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
    expect(h1.className).toContain("text-xl"); // PageTitle, FE-PAGE-7
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
    // `/^disable$/`, not `/disable/`: this row is `live`, so its menu also
    // carries "Move to Trash / Disable it first", whose accessible name a
    // loose match now finds too.
    expect(screen.getByRole("menuitem", { name: /^disable$/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /move to trash/i })).toBeInTheDocument();
    // Permanent delete is a TRASH action now — see the dedicated test below.
    expect(screen.queryByRole("menuitem", { name: /delete/i })).not.toBeInTheDocument();
  });

  it("offers a bulk action only once rows are selected", async () => {
    mockList({ data: { status: 200, data: [makeEndpoint({ id: "e1", name: "one", state: "draft" })] } });
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("one");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // Bulk mode is on but nothing is chosen: the action strip must not be there
    // at all, or the table jumps down the moment the mode is entered.
    expect(screen.queryByRole("button", { name: /to trash/i })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: /select one/i }));
    expect(screen.getByRole("button", { name: /move 1 to trash/i })).toBeInTheDocument();
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
  it("shows and selects a row whose folder_id is an orphan, because it is now on screen", async () => {
    // `f-missing` is not in the folders list — the row a concurrent folder
    // delete left behind. `groupByFolder` used to emit no bucket for it, so the
    // row rendered nowhere and "Select all" could sweep in something invisible.
    // It now falls into Uncategorized, so it is visible, selectable, and — with
    // the user looking straight at it — legitimately actionable.
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
    expect(screen.getByText("orphan row")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));

    // The count must describe what is in front of the user, not the filtered set.
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /move 2 to trash/i }));

    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-orphan" });
    expect(trashMutateAsync).toHaveBeenCalledTimes(2);
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

    await user.click(screen.getByRole("button", { name: /move 1 to trash/i }));

    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    expect(trashMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "e-hidden" });
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
 * These reverse the order. The bulk action is a move to Trash now, not the
 * hard DELETE it was when these were written — so the failure they pin is
 * recoverable where it used to be permanent. They are kept, and retargeted,
 * because what they actually guard is `selectedIds`' re-filter, which is
 * untouched by that change: acting on a row the user cannot see is the defect,
 * whatever the action then does.
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
    // The strip renders nothing at zero, so the action goes with it — there is
    // no way left to act on a row that is not on screen.
    expect(screen.queryByRole("button", { name: /to trash/i })).not.toBeInTheDocument();
  });

  it("trashes only the row still on screen when a selected row is hidden behind it", async () => {
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

    await user.click(screen.getByRole("button", { name: /move 1 to trash/i }));

    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "e-shown" });
    });
    // The one the user can no longer see must be left alone.
    expect(trashMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "e-hidden" });
    expect(trashMutateAsync).toHaveBeenCalledTimes(1);
  });
});

/**
 * A list whose rows change under an open dialog, and a mode whose only OFF
 * switch can unmount (final review, Minor #7).
 */
describe("Endpoints — the list changing underneath the page", () => {
  /** A list hook whose data is read fresh on every render, not frozen at mock time. */
  function mockListDynamic(get: () => DataApiEndpointListItemDto[]) {
    vi.mocked(api.useDataApiListEndpoints).mockImplementation(
      () => ({ data: { status: 200, data: get() }, isLoading: false, isError: false, error: null }) as unknown as
        ReturnType<typeof api.useDataApiListEndpoints>,
    );
  }

  // The "selection empties behind the open confirmation" test lived here. It
  // went with the bulk permanent delete: that dialog was its whole premise —
  // the race needed a step that stayed open across renders, and bulk Move to
  // Trash fires straight from the click against THIS render's `selectedIds`.
  // `doBulkTrash` keeps the `ids.length === 0` guard as defence, but there is
  // no longer a way to drive it, and a test that cannot reach the code it
  // names is worse than none.

  it("leaves bulk mode behind when the last endpoint goes, so the checkbox column does not return with the next one", async () => {
    let rows: DataApiEndpointListItemDto[] = [makeEndpoint({ id: "e1", name: "one", state: "draft" })];
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

/**
 * Bookmarks.
 *
 * Every fixture here has TWO rows in DIFFERENT bookmark states, deliberately.
 * A one-row fixture cannot tell "this star reflects THIS row" apart from "this
 * star reflects whether anything at all is bookmarked", and it cannot tell "the
 * click sent this row's id" apart from "the click sent a constant" — which is
 * exactly how a real defect survived four green tests one task ago.
 */
describe("Endpoints — bookmarks", () => {
  const STARRED = makeEndpoint({ id: "ep-star", name: "Starred report", bookmarked: true, state: "draft" });
  const PLAIN = makeEndpoint({ id: "ep-plain", name: "Plain report", bookmarked: false, state: "draft" });
  /** A second unbookmarked row: the peer that exposes an override leaking off its row. */
  const PLAIN_PEER = makeEndpoint({ id: "ep-plain-2", name: "Second plain report", bookmarked: false, state: "draft" });

  function renderTwoRows() {
    mockList({ data: { status: 200, data: [STARRED, PLAIN] } });
    return renderEndpoints();
  }

  it("gives each row a star that reads that row's own flag", async () => {
    renderTwoRows();
    await screen.findByText("Starred report");
    // Two rows, two different states, two different labels — a star driven by
    // "does the user have any bookmark" would make these two agree.
    expect(screen.getByRole("button", { name: "Remove bookmark from Starred report" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Bookmark Plain report" })).toHaveAttribute("aria-pressed", "false");
  });

  it("filters to bookmarked-only, hiding the unbookmarked row", async () => {
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmarks" }));

    expect(screen.getByText("Starred report")).toBeInTheDocument();
    expect(screen.queryByText("Plain report")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "true");

    // …and it is a TOGGLE: the hidden row comes back.
    await user.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("Plain report")).toBeInTheDocument();
  });

  it("says the list is empty because of the bookmark filter, not because of a facet", async () => {
    const user = userEvent.setup();
    mockList({ data: { status: 200, data: [PLAIN, makeEndpoint({ id: "ep-3", name: "Another plain" })] } });
    renderEndpoints();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("No bookmarked endpoints.")).toBeInTheDocument();
  });

  it("adds the bookmark for the row whose star was clicked", async () => {
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmark Plain report" }));

    await waitFor(() =>
      expect(bookmarkMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-plain" }),
    );
    // The direction is read from the row, so the OTHER mutation must stay put.
    expect(unbookmarkMutateAsync).not.toHaveBeenCalled();
  });

  it("removes the bookmark for the row whose filled star was clicked", async () => {
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Starred report");

    await user.click(screen.getByRole("button", { name: "Remove bookmark from Starred report" }));

    await waitFor(() =>
      expect(unbookmarkMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-star" }),
    );
    expect(bookmarkMutateAsync).not.toHaveBeenCalled();
  });

  it("surfaces a failed star instead of swallowing it", async () => {
    bookmarkMutateAsync.mockRejectedValue(new ApiError(500, "boom"));
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmark Plain report" }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
  });

  /**
   * The star is OPTIMISTIC.
   *
   * A click has to survive two round trips before the server can answer it —
   * the POST, then the refetch of the list the wrapper invalidated. These
   * tests hold the mutation open on purpose (`deferred()`), because everything
   * that goes wrong here goes wrong *during* that window and is invisible to a
   * test whose mock resolves immediately.
   */
  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }

  const OK = { status: 200, data: {}, headers: new Headers() };

  it("fills the star before the mutation resolves, and only the clicked row's", async () => {
    const inFlight = deferred<unknown>();
    bookmarkMutateAsync.mockReturnValue(inFlight.promise);
    const user = userEvent.setup();
    // THREE rows, and the third starts in the SAME state as the one clicked.
    // With only STARRED and PLAIN an override that leaked to every row would
    // be invisible — the leak sets `true`, and the row that could expose it is
    // already `true`. The peer that shares the clicked row's starting state is
    // what makes "only the clicked row's" an assertion rather than a wish.
    mockList({ data: { status: 200, data: [STARRED, PLAIN, PLAIN_PEER] } });
    renderEndpoints();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmark Plain report" }));

    // Still in flight — nothing has come back, and the star has already moved.
    expect(bookmarkMutateAsync).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Remove bookmark from Plain report" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    // …and the rows that were not clicked are exactly where they were. An
    // optimistic flip that leaked across rows would be the same defect as a
    // star reading "is anything bookmarked".
    expect(screen.getByRole("button", { name: "Bookmark Second plain report" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(screen.getByRole("button", { name: "Remove bookmark from Starred report" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    inFlight.resolve(OK);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Remove bookmark from Plain report" })).toBeInTheDocument(),
    );
  });

  it("puts the star back when the mutation is rejected", async () => {
    const inFlight = deferred<unknown>();
    bookmarkMutateAsync.mockReturnValue(inFlight.promise);
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmark Plain report" }));
    // The optimistic flip happened…
    expect(screen.getByRole("button", { name: "Remove bookmark from Plain report" })).toBeInTheDocument();

    inFlight.reject(new ApiError(500, "boom"));

    // …and it is taken back, because a flip with no rollback leaves the UI
    // asserting a bookmark the server never stored.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Bookmark Plain report" })).toHaveAttribute("aria-pressed", "false"),
    );
    expect(screen.queryByRole("button", { name: "Remove bookmark from Plain report" })).not.toBeInTheDocument();
    expect(toast.error).toHaveBeenCalled();
  });

  it("reverses on a second click instead of re-firing the first one", async () => {
    // The failure the review named: with a non-optimistic star, `bookmarked`
    // is still false while the POST is in flight, so an impatient second click
    // computes the same direction and adds the bookmark twice — the control
    // silently doing the opposite of what a toggle promises.
    const inFlight = deferred<unknown>();
    bookmarkMutateAsync.mockReturnValue(inFlight.promise);
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Plain report");

    await user.click(screen.getByRole("button", { name: "Bookmark Plain report" }));
    await user.click(screen.getByRole("button", { name: "Remove bookmark from Plain report" }));

    expect(bookmarkMutateAsync).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(unbookmarkMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-plain" }),
    );
    expect(screen.getByRole("button", { name: "Bookmark Plain report" })).toHaveAttribute("aria-pressed", "false");

    inFlight.resolve(OK);
  });

  it("drops an un-starred row out of the bookmarked-only view straight away", async () => {
    const inFlight = deferred<unknown>();
    unbookmarkMutateAsync.mockReturnValue(inFlight.promise);
    const user = userEvent.setup();
    renderTwoRows();
    await screen.findByText("Starred report");

    await user.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("Starred report")).toBeInTheDocument();

    // The filter reads the same override the glyph does, so the row leaves on
    // the click rather than a refetch later.
    await user.click(screen.getByRole("button", { name: "Remove bookmark from Starred report" }));
    expect(screen.queryByText("Starred report")).not.toBeInTheDocument();

    inFlight.resolve(OK);
  });
});

/**
 * Folders: creating one, and moving a row between them.
 *
 * Every fixture here has TWO folders and a row that is ALREADY IN ONE. With a
 * single folder, "moved to the right folder" and "moved to some folder" are the
 * same assertion, and a move that ignored its argument would pass; with a row
 * that starts folderless, "sent an explicit null" and "sent nothing" both leave
 * it where it was.
 */
describe("Endpoints — folders", () => {
  const REPORTING = { id: "f-1", name: "Reporting" };
  const ARCHIVE = { id: "f-2", name: "Archive" };
  const IN_REPORTING = makeEndpoint({ id: "ep-1", name: "Daily report", folder_id: "f-1" });
  const IN_ARCHIVE = makeEndpoint({ id: "ep-2", name: "Old export", folder_id: "f-2" });

  function renderTwoFolders() {
    mockList({ data: { status: 200, data: [IN_REPORTING, IN_ARCHIVE] } });
    mockFolders([REPORTING, ARCHIVE]);
    return renderEndpoints();
  }

  /** The one PATCH body the page sent, as the request object it really was. */
  function soleUpdateBody(): Record<string, unknown> {
    expect(updateMutateAsync).toHaveBeenCalledTimes(1);
    const [vars] = updateMutateAsync.mock.calls[0] as [{ workspace: string; id: string; data: Record<string, unknown> }];
    return vars.data;
  }

  it("moves a row into the folder that was chosen, not merely into a folder", async () => {
    const user = userEvent.setup();
    renderTwoFolders();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "Actions for Daily report" }));
    // The row's OWN folder is offered but inert — a live "Move to Reporting" on
    // a row already in Reporting is a request that changes nothing.
    expect(await screen.findByRole("menuitem", { name: 'Move to "Reporting"' })).toHaveAttribute("aria-disabled", "true");

    await user.click(screen.getByRole("menuitem", { name: 'Move to "Archive"' }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(updateMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-1", data: { folder_id: "f-2" } });
  });

  it("sends an explicit null — not an omitted key — when moving to Uncategorized", async () => {
    const user = userEvent.setup();
    renderTwoFolders();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "Actions for Daily report" }));
    await user.click(await screen.findByRole("menuitem", { name: /remove from folder/i }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const body = soleUpdateBody();
    // `folder_id` is `Option<Option<Uuid>>` server-side: an ABSENT key means
    // "leave the folder alone" and `null` means "clear it". `{}` and
    // `{folder_id: null}` are therefore different requests with opposite
    // meanings, and only the key's PRESENCE tells them apart.
    expect(Object.prototype.hasOwnProperty.call(body, "folder_id")).toBe(true);
    expect(body.folder_id).toBeNull();
    // …and nothing else rides along: a PATCH carrying the row's name or sql
    // would re-submit whatever the list last fetched for them.
    expect(Object.keys(body)).toEqual(["folder_id"]);
  });

  it("does not offer Remove from folder to a row that is already uncategorized", async () => {
    mockList({ data: { status: 200, data: [IN_REPORTING, makeEndpoint({ id: "ep-3", name: "Loose one", folder_id: null })] } });
    mockFolders([REPORTING, ARCHIVE]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Loose one");

    await user.click(screen.getByRole("button", { name: "Actions for Loose one" }));
    await screen.findByRole("menuitem", { name: 'Move to "Archive"' });
    expect(screen.queryByRole("menuitem", { name: /remove from folder/i })).not.toBeInTheDocument();
  });

  it("creates a folder from the toolbar, and the list groups under it", async () => {
    const user = userEvent.setup();
    const { rerenderTree } = renderTwoFolders();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "New folder" }));
    await user.type(await screen.findByPlaceholderText("Folder name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(foldersApi.foldersFoldersCreate).toHaveBeenCalledWith("eng", "data_api_endpoints", { name: "Ops" });
    });
    // Creating a folder must not move anything.
    expect(updateMutateAsync).not.toHaveBeenCalled();
    expect(toast.success).toHaveBeenCalledWith("Folder created");

    // The refetch the invalidation triggers, played back: the new folder is a
    // section header of its own, alongside the two that were already there.
    mockFolders([REPORTING, ARCHIVE, { id: "f-new", name: "Ops" }]);
    rerenderTree();
    expect(await screen.findByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Reporting")).toBeInTheDocument();
  });

  it("moves the row whose menu was used into a brand-new folder", async () => {
    const user = userEvent.setup();
    renderTwoFolders();
    await screen.findByText("Old export");

    // The SECOND row's menu, with a peer in the same starting state one row
    // above it: a handler that moved "the first row" would look identical here
    // if only one row existed.
    await user.click(screen.getByRole("button", { name: "Actions for Old export" }));
    await user.click(await screen.findByRole("menuitem", { name: /move to new folder/i }));
    await user.type(await screen.findByPlaceholderText("Folder name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Move" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(foldersApi.foldersFoldersCreate).toHaveBeenCalledWith("eng", "data_api_endpoints", { name: "Ops" });
    // `f-new` is what the create resolved with — the id has to come from the
    // response, not from the name that was typed.
    expect(updateMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-2", data: { folder_id: "f-new" } });
  });

  it("reports a failed move rather than leaving the row looking moved", async () => {
    updateMutateAsync.mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    renderTwoFolders();
    await screen.findByText("Daily report");

    await user.click(screen.getByRole("button", { name: "Actions for Daily report" }));
    await user.click(await screen.findByRole("menuitem", { name: 'Move to "Archive"' }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.success).not.toHaveBeenCalled();
  });
});

/**
 * Folder SECTION HEADERS, which are not the same thing as folder grouping
 * (task 7, item 1).
 *
 * The live audit found a workspace holding one endpoint and no folders at all
 * rendering "▸ Uncategorized (1)" above its single row: a taxonomy the
 * workspace does not have, spelled out as a collapsible section, over a list
 * that is not divided into anything. "Uncategorized" carries information only
 * beside a NAMED folder — alone it is the whole list wearing a label, and a
 * collapse control whose only effect is to hide everything.
 *
 * Every fixture below comes in a with-folders / without-folders PAIR. A block
 * that only ever mocked `mockFolders([])` could not tell "no header when there
 * are no folders" apart from "no headers, ever", and the second is a
 * regression that would hide every named folder on the page.
 *
 * This is deliberately AHEAD of `Charts.tsx`, `Datasets.tsx`, `Dashboards.tsx`,
 * `Notebooks.tsx` and `Alerts.tsx`, which all render the bucket's header
 * unconditionally — the same relationship §1.5 of `docs/en/ui-conventions.md`
 * records for `role="rowheader"` on `Clients.tsx`. See the comment on
 * `showFolderHeaders` in `Endpoints.tsx`.
 */
describe("Endpoints — a folder header needs a folder", () => {
  const REPORTING = { id: "f-1", name: "Reporting" };
  const LOOSE_A = makeEndpoint({ id: "ep-a", name: "Ad hoc query", folder_id: null });
  const LOOSE_B = makeEndpoint({ id: "ep-b", name: "Second query", folder_id: null });
  const IN_REPORTING = makeEndpoint({ id: "ep-r", name: "Daily report", folder_id: "f-1" });


  it("heads no section at all in a workspace with no folders", async () => {
    mockList({ data: { status: 200, data: [LOOSE_A] } });
    mockFolders([]);
    renderEndpoints();

    // The row is there…
    expect(await screen.findByText("Ad hoc query")).toBeInTheDocument();
    // …and under nothing. `Toggle Uncategorized` is `FolderHeaderRow`'s own
    // accessible name, so this asks whether the HEADER ROW exists rather than
    // whether the word happens to appear somewhere on the page.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
    expect(screen.queryByText("Uncategorized")).not.toBeInTheDocument();
  });

  it("still heads Uncategorized once there is a real folder to contrast it with", async () => {
    mockList({ data: { status: 200, data: [IN_REPORTING, LOOSE_A] } });
    mockFolders([REPORTING]);
    renderEndpoints();

    expect(await screen.findByRole("button", { name: "Toggle Reporting" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeInTheDocument();
    // Both sections' rows, so this is not passing on an empty table.
    expect(screen.getByText("Daily report")).toBeInTheDocument();
    expect(screen.getByText("Ad hoc query")).toBeInTheDocument();
  });

  it("collapses the Uncategorized section wherever it IS headed", async () => {
    mockList({ data: { status: 200, data: [IN_REPORTING, LOOSE_A] } });
    mockFolders([REPORTING]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    await user.click(screen.getByRole("button", { name: "Toggle Uncategorized" }));

    expect(screen.queryByText("Ad hoc query")).not.toBeInTheDocument();
    // The named folder is untouched by that click — a collapse that took the
    // whole table with it would satisfy the line above just as well.
    expect(screen.getByText("Daily report")).toBeInTheDocument();
  });

  it("keeps the header up while a filter empties every named folder", async () => {
    // The alternative gate — "more than one group" — reads the same as
    // `folders.length > 0` on every fixture above, and differs here:
    // `groupByFolder` drops empty NAMED folders while a filter is engaged, so
    // a search matching only folderless rows collapses `groups` to one. Gated
    // on that, the header would blink out mid-keystroke and back on deleting
    // the last character, in a workspace whose folders never went anywhere.
    mockList({ data: { status: 200, data: [IN_REPORTING, LOOSE_A] } });
    mockFolders([REPORTING]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    await user.type(screen.getByPlaceholderText("Search endpoints…"), "Ad hoc");

    // The named folder has no matching rows left and is gone from the table…
    await waitFor(() => expect(screen.queryByText("Daily report")).not.toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Toggle Reporting" })).not.toBeInTheDocument();
    // …but the workspace still has folders, so the surviving section keeps its
    // heading rather than losing it for the length of a search.
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeInTheDocument();
  });

  it("does not strand rows behind a collapse state whose header is no longer rendered", async () => {
    // What a previous session left behind: Uncategorized collapsed, from back
    // when this workspace still had folders. `useCollapsed` reads that out of
    // localStorage at mount, and it survives the folders being deleted.
    // Both halves derived, never spelled out: `folderCollapseKey` is the same
    // function the component hands `useCollapsed`, and `UNCATEGORIZED_KEY` the
    // same constant it collapses under. A hardcoded key would survive a change
    // to either format by landing the seed where nothing reads it — and this
    // test would still PASS, because a seed that misses is indistinguishable
    // from a page that correctly ignored it. That is the failure mode that
    // stops the M2 mutant (render gated, `visibleRows` not) from dying.
    localStorage.setItem(folderCollapseKey("eng"), JSON.stringify([UNCATEGORIZED_KEY]));
    mockList({ data: { status: 200, data: [LOOSE_A, LOOSE_B] } });
    mockFolders([]);
    const user = userEvent.setup();
    renderEndpoints();

    // Rendering the rows is only half of it. `visibleRows` — what bulk select
    // acts on — is derived from `groups` and the SAME collapse state in a
    // separate `useMemo`, so dropping the header in the render loop alone
    // leaves "Select all" sweeping zero rows while two sit in front of the
    // user, with no header left anywhere to expand.
    expect(await screen.findByText("Ad hoc query")).toBeInTheDocument();
    expect(screen.getByText("Second query")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));

    expect(screen.getByText("2 selected")).toBeInTheDocument();
  });
});

/**
 * The header's four icon-only controls (task 7, item 2).
 *
 * They stay icon-only: §1.2 of `docs/en/ui-conventions.md` fixes the order and
 * the Explore idiom keeps these compact, so everything that tells them apart
 * is either a glyph or an accessible name.
 *
 * The name half is asserted through `getByRole(…, { name })`, which runs the
 * REAL accessible-name computation. A hand-rolled `aria-label ?? textContent`
 * read agrees with it on today's markup and disagrees the moment one of these
 * is named by another element — a prior task in this programme shipped that
 * approximation. It is also why these assertions are not written against
 * `aria-hidden` on the glyph: `lucide-react` sets that itself, so such a test
 * would pin the library rather than this page.
 *
 * jsdom does no layout, so nothing here claims anything about SPACING. What it
 * can see is order and structure: a `separator` element standing between the
 * view controls and the destructive one, and no separator where there is
 * nothing to its left.
 */
describe("Endpoints — the header's icon controls", () => {
  const TRASHED = makeEndpoint({
    id: "ep-t", name: "Trashed alpha", deleted_at: "2026-08-18T09:00:00Z",
  } as Partial<DataApiEndpointListItemDto>);

  /** The `<header>` `ListPageShell` puts the title and both slots in. */
  function pageHeader(): HTMLElement {
    const el = screen.getByRole("heading", { level: 1 }).closest("header");
    if (!el) throw new Error("ListPageShell renders its title inside a <header>");
    return el;
  }

  it("gives every one of them an accessible name", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    mockFolders([]);
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    const h = pageHeader();
    for (const name of ["Bookmarks", "Bulk select", "New folder", "Trash"]) {
      expect(within(h).getByRole("button", { name })).toBeInTheDocument();
    }
  });

  it("puts a separator between the view controls and the destructive one", async () => {
    mockList({ data: { status: 200, data: [LOOSE_ROW] } });
    mockFolders([]);
    renderEndpoints();
    await screen.findByText("Ad hoc query");

    const h = pageHeader();
    const separator = within(h).getByRole("separator");
    const newFolder = within(h).getByRole("button", { name: "New folder" });
    const trash = within(h).getByRole("button", { name: "Trash" });

    // Existence alone would be satisfied by a divider parked anywhere in the
    // header, including after Trash — the whole point is WHICH controls it
    // stands between.
    expect(newFolder.compareDocumentPosition(separator) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(separator.compareDocumentPosition(trash) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  });

  it("renders no separator over an empty list, where nothing stands to its left", async () => {
    mockList({ data: { status: 200, data: [] } });
    mockFolders([]);
    renderEndpoints();
    await screen.findByText(/no endpoints yet/i);

    const h = pageHeader();
    // The view controls are gated on `hasData`; Trash is not (§1.2), so it is
    // alone up there and a divider would be separating it from nothing.
    expect(within(h).getByRole("button", { name: "Trash" })).toBeInTheDocument();
    expect(within(h).queryByRole("separator")).not.toBeInTheDocument();
  });

  it("renders no separator in the trash either, which offers none of the view controls", async () => {
    mockListByView([LOOSE_ROW], [TRASHED]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Ad hoc query");
    // Present in the normal view — otherwise its absence below proves nothing.
    expect(within(pageHeader()).getByRole("separator")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");

    expect(within(pageHeader()).queryByRole("separator")).not.toBeInTheDocument();
  });
});

/**
 * The trash (P6, task 4).
 *
 * Two things make these tests able to fail, and both were deliberate:
 *
 *  - the fixture answers the `trashed` PARAM (`mockListByView`), so a toggle
 *    that repaints the same rows instead of re-querying is visible; and
 *  - every fixture that exercises the live-state guard carries a `live` row AND
 *    a non-`live` one. With only a `live` row, "Move to Trash is disabled" is
 *    equally satisfied by an implementation that disables it for everything —
 *    the two are literally the same rendering.
 *
 * Payloads, never call counts: `expect(mock).toHaveBeenCalled()` is true when
 * the wrong row's id was sent, which on a Restore is the difference between
 * bringing back the endpoint the author clicked and bringing back another one.
 */
describe("Endpoints — trash", () => {
  const LIVE_ROW = makeEndpoint({ id: "ep-live", name: "Live report", state: "live", live_version_id: "v-1" });
  const DRAFT_ROW = makeEndpoint({ id: "ep-draft", name: "Draft report", state: "draft" });
  const TRASHED_A = makeEndpoint({
    id: "ep-trash-a", name: "Trashed alpha", state: "disabled", live_version_id: "v-9",
    deleted_at: "2026-08-18T09:00:00Z",
  } as Partial<DataApiEndpointListItemDto>);
  const TRASHED_B = makeEndpoint({
    id: "ep-trash-b", name: "Trashed beta", state: "draft",
    deleted_at: "2026-08-18T10:00:00Z",
  } as Partial<DataApiEndpointListItemDto>);

  it("asks the API for the trashed LIST, rather than filtering the live one", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");

    // The default view is an explicit `trashed: false`, not an absent param:
    // the two views have to be separate cache entries or the toggle would
    // serve one from the other.
    expect(api.useDataApiListEndpoints).toHaveBeenCalledWith("eng", { trashed: false });
    expect(api.useDataApiListEndpoints).not.toHaveBeenCalledWith("eng", { trashed: true });

    await user.click(screen.getByRole("button", { name: "Trash" }));

    expect(api.useDataApiListEndpoints).toHaveBeenCalledWith("eng", { trashed: true });
    // …and the rows really are the other list's, not the same ones relabelled.
    expect(await screen.findByText("Trashed alpha")).toBeInTheDocument();
    expect(screen.queryByText("Draft report")).not.toBeInTheDocument();
  });

  it("renames the page's h1 to Trash, and the toggle reports itself pressed", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Endpoints");

    await user.click(screen.getByRole("button", { name: "Trash" }));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Trash");
    // `aria-pressed` is the whole reason this toggle uses `ListToolbarButton`
    // rather than a fifth hand-written Button+Tooltip pair.
    expect(screen.getByRole("button", { name: "Trash" })).toHaveAttribute("aria-pressed", "true");
  });

  it("drops the star, the bulk toggle, the folder control and + Endpoint in the trash", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    // All four are present in the normal view — otherwise their absence in the
    // trash would prove nothing about the trash.
    expect(await screen.findByRole("button", { name: "Bookmark Draft report" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Bulk select" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New folder" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "+ Endpoint" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");

    expect(screen.queryByRole("button", { name: /^Bookmark / })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Bulk select" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New folder" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "+ Endpoint" })).not.toBeInTheDocument();
    // The Bookmarks FILTER goes with them; so do the two facets.
    expect(screen.queryByRole("button", { name: "Bookmarks" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "State" })).not.toBeInTheDocument();
  });

  it("leaves the checkbox column behind even if bulk mode was on when the trash was opened", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    expect(screen.getByRole("checkbox", { name: "Select all" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");
    // A `bulkMode` left true here would strand a checkbox column whose only
    // OFF switch is a toggle the trash does not render.
    expect(screen.queryByRole("checkbox", { name: "Select all" })).not.toBeInTheDocument();
  });

  it("keeps the way OUT of an empty trash", async () => {
    mockListByView([DRAFT_ROW], []);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");

    await user.click(screen.getByRole("button", { name: "Trash" }));

    expect(await screen.findByText("Trash is empty.")).toBeInTheDocument();
    // The toolbar is gated on `hasData` for filters — the Trash toggle is not,
    // or it would vanish exactly when it is the only control left.
    expect(screen.getByRole("button", { name: "Trash" })).toBeInTheDocument();
    // And an empty trash is not a first-run workspace.
    expect(screen.queryByText(/no endpoints yet/i)).not.toBeInTheDocument();
  });

  /**
   * The Trash's last column, pinned two ways: the HEADER it is under, and the
   * timestamp the CELL renders.
   *
   * Neither half was covered, and the gap was not theoretical — an
   * implementation that kept rendering `updated_at` under a `Trashed` heading
   * passed all 237 tests in the frontend suite. That is why the fixture's two
   * timestamps must differ: `TRASHED_A` was last edited on Aug 10 and trashed on
   * Aug 18, so a cell reading the wrong field renders a visibly different value
   * rather than the same one by luck.
   */
  it("the Trash's last column is Trashed, and it renders deleted_at rather than updated_at", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");
    // The normal view first, so the swap below is a swap rather than the only
    // column this page has ever rendered.
    expect(screen.getByText("Last edited")).toBeInTheDocument();
    expect(screen.queryByText("Trashed")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Trash" }));
    const row = (await screen.findByText("Trashed alpha")).closest("tr");
    expect(row).not.toBeNull();

    expect(screen.getByText("Trashed")).toBeInTheDocument();
    expect(screen.queryByText("Last edited")).not.toBeInTheDocument();
    // `deleted_at` is on screen…
    expect(
      within(row as HTMLElement).getByTitle(fullTimestamp("2026-08-18T09:00:00Z")),
    ).toBeInTheDocument();
    // …and `updated_at` is NOT. Without this half, a cell rendering both — or
    // the wrong one under a right-looking header — still passes.
    expect(
      within(row as HTMLElement).queryByTitle(fullTimestamp("2026-08-10T00:00:00Z")),
    ).not.toBeInTheDocument();
  });

  /**
   * The Trash is ordered by the column it SHOWS.
   *
   * The two rows are built to disagree: alpha was edited more recently but
   * trashed first, beta was edited long ago but trashed last. Under the old
   * default (`updated_at`, desc, carried unchanged into the trash) the order was
   * alpha-then-beta while the visible Trashed column read 09:00 above 10:00 —
   * a table sorted by a column that is not on screen, and therefore an order the
   * author cannot argue with. Any fixture where the two agree would pass either
   * way.
   */
  it("sorts the Trash by when rows were trashed, not by when they were last edited", async () => {
    const EDITED_LAST_TRASHED_FIRST = makeEndpoint({
      id: "ep-trash-a", name: "Trashed alpha", state: "draft",
      updated_at: "2026-08-17T00:00:00Z", deleted_at: "2026-08-18T09:00:00Z",
    } as Partial<DataApiEndpointListItemDto>);
    const EDITED_FIRST_TRASHED_LAST = makeEndpoint({
      id: "ep-trash-b", name: "Trashed beta", state: "draft",
      updated_at: "2026-07-01T00:00:00Z", deleted_at: "2026-08-18T10:00:00Z",
    } as Partial<DataApiEndpointListItemDto>);
    // Server order is the WRONG order, so a page that merely passes the payload
    // through fails rather than coincidentally agreeing.
    mockListByView([DRAFT_ROW], [EDITED_LAST_TRASHED_FIRST, EDITED_FIRST_TRASHED_LAST]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Draft report");

    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed beta");

    const names = () =>
      screen
        .getAllByRole("row")
        .slice(1)
        .map((r) => within(r).getByRole("link").textContent);
    // Most recently trashed first — the reverse of `updated_at` desc, which is
    // what the page used to do here.
    expect(names()).toEqual(["Trashed beta", "Trashed alpha"]);
    // And the header says so, rather than the order being an accident of the
    // payload: `aria-sort` sits on the column the rows are actually ordered by.
    expect(screen.getByText("Trashed").closest("th")).toHaveAttribute("aria-sort", "descending");

    // The header drives the same key it advertises — clicking it reverses the
    // order it claims, which a decorative `aria-sort` would not.
    await user.click(screen.getByText("Trashed"));
    expect(names()).toEqual(["Trashed alpha", "Trashed beta"]);
    expect(screen.getByText("Trashed").closest("th")).toHaveAttribute("aria-sort", "ascending");

    // Leaving the trash hands the sort back to the column THAT view displays —
    // a `deleted_at` sort stranded on the normal list is the same defect
    // mirrored, and no header there would own it.
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Draft report");
    expect(screen.getByText("Last edited").closest("th")).toHaveAttribute("aria-sort", "descending");
  });

  it("Restore sends the id of the row whose menu was used, not the first row's", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A, TRASHED_B]);
    const user = userEvent.setup();
    renderEndpoints();
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed beta");

    await user.click(screen.getByRole("button", { name: "Actions for Trashed beta" }));
    await user.click(await screen.findByRole("menuitem", { name: "Restore" }));

    // The PAYLOAD. Two trashed rows exist precisely so a hard-coded or
    // first-row id is a failure rather than a coincidence.
    await waitFor(() => {
      expect(restoreMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-trash-b" });
    });
    expect(restoreMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "ep-trash-a" });
    // Restore returns the endpoint as `disabled`, never as whatever it was
    // before; the toast is where the author is told so.
    await waitFor(() => {
      expect(vi.mocked(toast.success)).toHaveBeenCalledWith("Endpoint restored as disabled");
    });
  });

  it("offers Restore and Delete permanently in the trash, and none of the live actions", async () => {
    mockListByView([DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");

    await user.click(screen.getByRole("button", { name: "Actions for Trashed alpha" }));
    expect(await screen.findByRole("menuitem", { name: "Restore" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Delete permanently" })).toBeInTheDocument();
    // `TRASHED_A` is `disabled` WITH a `live_version_id`, so in the normal view
    // its menu would offer a clickable Enable — task 3 made that a 409 for a
    // trashed endpoint, so it must not be offered here.
    expect(screen.queryByRole("menuitem", { name: /^enable$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /move to trash/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /^open$/i })).not.toBeInTheDocument();
  });

  /**
   * Permanent delete belongs to the trash and to nowhere else — the same shape
   * as `DatasetActionsMenu`, and the reason the whole phase exists. Offered
   * beside Move to Trash it let an author destroy a served URL and its version
   * history without passing through the trash at all, and since trash REFUSES
   * a `live` endpoint it was the easier of the two paths on exactly the rows
   * where it is most destructive.
   *
   * BOTH halves are in one test on purpose. "Delete permanently is in the trash
   * menu" is satisfied by an implementation that puts it in every menu, so the
   * absence assertions are what carry the claim — and they run over a `live`
   * row AND a `draft` row, because an item removed for only one state would
   * otherwise pass.
   */
  it("offers permanent delete ONLY from the trash — not from a live row, not from a draft one", async () => {
    mockListByView([LIVE_ROW, DRAFT_ROW], [TRASHED_A]);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Live report");

    await user.click(screen.getByRole("button", { name: "Actions for Live report" }));
    // `/delete/i`, deliberately loose: it also catches a "Delete permanently"
    // leaking into the live menu, which an anchored `/^delete$/` would miss.
    expect(await screen.findByRole("menuitem", { name: /move to trash/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /delete/i })).not.toBeInTheDocument();

    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Actions for Draft report" }));
    expect(await screen.findByRole("menuitem", { name: /move to trash/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /delete/i })).not.toBeInTheDocument();

    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");
    await user.click(screen.getByRole("button", { name: "Actions for Trashed alpha" }));
    expect(await screen.findByRole("menuitem", { name: "Delete permanently" })).toBeInTheDocument();
  });

  it("disables Move to Trash for a live row and says why, while the draft beside it can be trashed", async () => {
    // ONE fixture, BOTH states. With only the live row, an implementation that
    // disables Move to Trash unconditionally renders identically.
    mockListByView([LIVE_ROW, DRAFT_ROW], []);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Live report");

    await user.click(screen.getByRole("button", { name: "Actions for Live report" }));
    const blocked = await screen.findByRole("menuitem", { name: /move to trash/i });
    expect(blocked).toHaveAttribute("aria-disabled", "true");
    // Visible, not a `title` — the rule is only taught if it is on screen.
    expect(screen.getByText("Disable it first")).toBeInTheDocument();
    // Disabled, so nothing is sent even in jsdom, where the CSS that blocks
    // the click does not apply.
    await user.click(blocked);
    expect(trashMutateAsync).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Actions for Draft report" }));
    const allowed = await screen.findByRole("menuitem", { name: /move to trash/i });
    expect(allowed).not.toHaveAttribute("aria-disabled", "true");
    expect(screen.queryByText("Disable it first")).not.toBeInTheDocument();

    await user.click(allowed);
    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-draft" });
    });
    expect(trashMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "ep-live" });
  });

  it("disables bulk Move to Trash while a live row is selected, and says how many", async () => {
    mockListByView([LIVE_ROW, DRAFT_ROW], []);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Live report");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select Draft report" }));
    // Draft only: the action is live, which is what makes the next assertion
    // about the live row mean something.
    expect(screen.getByRole("button", { name: /move 1 to trash/i })).toBeEnabled();

    await user.click(screen.getByRole("checkbox", { name: "Select Live report" }));
    expect(screen.getByRole("button", { name: /move 2 to trash/i })).toBeDisabled();
    // The COUNT, so the author knows how much of the selection to go and disable.
    expect(screen.getByText("1 live — disable it first")).toBeInTheDocument();

    // Un-select the live row and the action comes back — a guard that latched
    // would pass every assertion above.
    await user.click(screen.getByRole("checkbox", { name: "Select Live report" }));
    await user.click(screen.getByRole("button", { name: /move 1 to trash/i }));
    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-draft" });
    });
    expect(trashMutateAsync).not.toHaveBeenCalledWith({ workspace: "eng", id: "ep-live" });
    expect(trashMutateAsync).toHaveBeenCalledTimes(1);
  });

  /**
   * Move to Trash is the ONLY bulk action, the same as `Datasets.tsx`,
   * `Charts.tsx` and `Dashboards.tsx`.
   *
   * A bulk permanent delete used to sit beside it and undid single-row
   * deletion's whole reason for moving behind the trash — it destroyed served
   * URLs and their published version history in one click, `live` rows
   * included. This pins its absence rather than trusting the edit: the strip is
   * exactly where it would come back, and it came back once already in the form
   * of the row menu's `Delete`.
   *
   * Asserted as "no destructive-delete control in the strip", not as "the
   * button named `Delete selected` is gone" — the latter passes against a
   * re-added button called anything else.
   */
  it("offers Move to Trash and no permanent delete as a bulk action", async () => {
    mockListByView([DRAFT_ROW, makeEndpoint({ id: "ep-d2", name: "Second draft", state: "draft" })], []);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Second draft");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));

    expect(screen.getByText("2 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /move 2 to trash/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    // And no confirmation dialog is waiting behind one either.
    expect(screen.queryByPlaceholderText("delete")).not.toBeInTheDocument();
  });

  it("bulk Move to Trash trashes every selected row and reports the number", async () => {
    const D2 = makeEndpoint({ id: "ep-draft-2", name: "Second draft", state: "draft" });
    mockListByView([DRAFT_ROW, D2], []);
    const user = userEvent.setup();
    renderEndpoints();
    await screen.findByText("Second draft");

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: /move 2 to trash/i }));

    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-draft" });
    });
    expect(trashMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-draft-2" });
    await waitFor(() => {
      expect(vi.mocked(toast.success)).toHaveBeenCalledWith("Moved 2 endpoints to Trash");
    });
  });

  /**
   * Invalidation, driven through the `onSuccess` `src/api/dataApi.ts` attaches
   * — the same way the `cache invalidation` block above pins the others.
   *
   * The key asserted is the bare PREFIX, with no params object. That is the
   * load-bearing part: react-query matches by prefix, so `["…", ws]` reaches
   * BOTH `["…", ws]` (the normal list) and `["…", ws, {trashed: true}]` (the
   * trash), while the parameterised key would reach only one of them. A
   * restore that refreshed only the trash would leave the restored endpoint
   * invisible in the list it was restored INTO until a full reload.
   */
  const INVALIDATION_RESULT = { status: 200, data: {}, headers: new Headers() } as never;
  async function fireSuccessOn(hook: { mock: { calls: unknown[][] } }, variables: Record<string, unknown>) {
    const opts = hook.mock.calls.at(-1)?.[0] as
      | { mutation?: { onSuccess?: (...a: never[]) => unknown } }
      | undefined;
    await opts?.mutation?.onSuccess?.(
      INVALIDATION_RESULT, variables as never, undefined as never, undefined as never,
    );
  }

  it("Trash invalidates BOTH lists, so the row leaves one view and appears in the other", async () => {
    mockListByView([DRAFT_ROW], []);
    const { qc } = renderEndpoints();
    await screen.findByText("Draft report");
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccessOn(vi.mocked(api.useDataApiTrashEndpoint), { workspace: "eng", id: "ep-draft" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
    // The endpoint RECORD too: a detail tab open on it must not keep claiming
    // the endpoint is present and editable.
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiGetEndpoint", "eng", "ep-draft"] }),
    );
  });

  it("Restore invalidates BOTH lists, so the restored row is not stuck in the trash", async () => {
    mockListByView([], [TRASHED_A]);
    const user = userEvent.setup();
    const { qc } = renderEndpoints();
    // The restore mutation is constructed per rendered ROW, so the row has to
    // be on screen or `fireSuccessOn` fires nothing and the test passes empty.
    await user.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("Trashed alpha");

    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccessOn(vi.mocked(api.useDataApiRestoreEndpoint), { workspace: "eng", id: "ep-trash-a" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiGetEndpoint", "eng", "ep-trash-a"] }),
    );
  });
});

/**
 * The strip counts rows, so a one-row workspace is the common first-run case,
 * not an edge one — and it rendered "1 endpoints total".
 */
describe("Endpoints — the summary strip agrees with itself about number", () => {
  it("counts one endpoint without an s", async () => {
    mockList({ data: { status: 200, data: [makeEndpoint({ id: "e1", name: "only one" })] } });
    renderEndpoints();
    await screen.findByText("only one");

    // `getByText` matches on an element's own text nodes, so this finds the
    // strip item and `.textContent` then reads the number beside it — the
    // sentence as a user sees it, not the label constant.
    expect(screen.getByText("endpoint total").textContent).toBe("1endpoint total");
    expect(screen.queryByText("endpoints total")).not.toBeInTheDocument();
  });

  it("keeps the s for two", async () => {
    // Guards the other direction: a naive `s`-stripper would render
    // "2 endpoint total".
    mockList({
      data: {
        status: 200,
        data: [makeEndpoint({ id: "e1", name: "one" }), makeEndpoint({ id: "e2", name: "two" })],
      },
    });
    renderEndpoints();
    await screen.findByText("one");

    expect(screen.getByText("endpoints total").textContent).toBe("2endpoints total");
  });

  it("never renders a zero count — the empty workspace gets the first-run CTA instead", async () => {
    // The third branch, and the reason "0 endpoints total" is not asserted
    // anywhere: `ListStates` swaps the whole list for its `empty` slot, so the
    // strip is not on screen at all. Both halves are asserted — the CTA is
    // present AND the strip is absent — because the CTA alone would still pass
    // over a page that rendered both.
    mockList({ data: { status: 200, data: [] } });
    renderEndpoints();

    expect(await screen.findByText(/no endpoints yet/i)).toBeInTheDocument();
    expect(screen.queryByText("endpoints total")).not.toBeInTheDocument();
    expect(screen.queryByText("endpoint total")).not.toBeInTheDocument();
  });
});

// KI-023. Serving matches the LIVE route, so a path saved on a live endpoint's
// draft has not moved what callers call. The list shows the URL callers use,
// with the draft's new one labelled as waiting for a publish.
describe("Endpoints — a draft path that has not gone live yet", () => {
  it("shows the live URL and labels the draft's new one as pending", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({
            state: "live",
            live_version_id: "v-1",
            path: "/orders/lookup",
            live_path: "/x/probe",
            live_method: "GET",
          }),
        ],
      },
    });
    renderEndpoints();

    const row = (await screen.findByText("Daily report")).closest("tr")!;
    expect(within(row).getByText("/x/probe")).toBeInTheDocument();
    const pending = within(row).getByTestId("endpoint-pending-route-ep-1");
    expect(pending).toHaveTextContent("Pending publish: GET /orders/lookup");
  });

  it("says nothing is pending when the draft is at the live URL", async () => {
    mockList({
      data: {
        status: 200,
        data: [
          makeEndpoint({ state: "live", live_version_id: "v-1", path: "/reports/daily", live_path: "/reports/daily", live_method: "GET" }),
        ],
      },
    });
    renderEndpoints();

    const row = (await screen.findByText("Daily report")).closest("tr")!;
    expect(within(row).getByText("/reports/daily")).toBeInTheDocument();
    expect(within(row).queryByText(/pending publish/i)).not.toBeInTheDocument();
  });
});
