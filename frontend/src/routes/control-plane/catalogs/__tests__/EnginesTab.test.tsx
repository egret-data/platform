import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, useMutation } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * Control Plane → Catalogs → detail → Engines.
 *
 * The registry module is mocked **partially** (`importOriginal`, then the four
 * hooks this tab calls) so the query-key BUILDERS stay real: the tab
 * invalidates `catalogKey(id)`, which delegates to one of them, and a literal
 * in its place would leave a detached engine on screen with every test here
 * still green — the defect Task 4's review found on the list page.
 *
 * Each mutation hook runs a REAL `useMutation` over a stub `mutationFn`, so
 * the `onSuccess`/`onError` the tab attaches actually fire. A hook mocked to a
 * bare object would swallow both, and the 201-but-`failed` toast — the whole
 * point of one of these tests — could not be observed at all.
 */
type RegistryModule = typeof import("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
type EnginesModule = typeof import("@/api/generated/control-plane-engines/control-plane-engines");

vi.mock(
  "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry",
  async (importOriginal) => ({
    ...(await importOriginal<RegistryModule>()),
    useControlPlaneCatalogRegistryRegistryAttachCatalog: vi.fn(),
    useControlPlaneCatalogRegistryDetachCatalog: vi.fn(),
    useControlPlaneCatalogRegistryRetryCatalogApply: vi.fn(),
    useControlPlaneCatalogRegistryTestRegistryCatalog: vi.fn(),
  }),
);
vi.mock("@/api/generated/control-plane-engines/control-plane-engines", async (importOriginal) => ({
  ...(await importOriginal<EnginesModule>()),
  useControlPlaneEnginesListEngines: vi.fn(),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as enginesApi from "@/api/generated/control-plane-engines/control-plane-engines";
import type {
  CatalogAttachmentDto,
  CatalogTestResultDto,
  EngineDto,
  RegistryCatalogDto,
} from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { TooltipProvider } from "@/components/ui/tooltip";
import { toast } from "sonner";
import { EnginesTab } from "@/routes/control-plane/catalogs/EnginesTab";
import { catalogKey } from "@/routes/control-plane/queryKeys";

const ADMIN: Me = {
  user_id: "u1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};
const READER: Me = {
  user_id: "u2",
  email: "reader@example.com",
  platform_roles: [],
  permissions: ["catalog_registry:read"],
};

function attachment(over: Partial<CatalogAttachmentDto> = {}): CatalogAttachmentDto {
  return {
    engineId: "e1",
    engineName: "trino-prod",
    engineType: "trino",
    applyStatus: "applied",
    applyError: null,
    appliedAt: "2026-09-01T00:00:00Z",
    isPrimary: false,
    createdAt: "2026-09-01T00:00:00Z",
    ...over,
  };
}

function catalog(over: Partial<RegistryCatalogDto> = {}): RegistryCatalogDto {
  return {
    id: "c-1",
    name: "sales",
    kind: "jdbc",
    config: { dialect: "postgresql", host: "db.internal", port: 5432 },
    advanced: {},
    description: null,
    warehouseId: null,
    engines: [attachment()],
    workspaceIds: [],
    createdAt: "2026-08-01T10:00:00Z",
    updatedAt: "2026-09-01T10:00:00Z",
    ...over,
  };
}

function engine(over: Partial<EngineDto> = {}): EngineDto {
  return {
    id: "e1",
    name: "trino-prod",
    type: "trino",
    namespace: "egret",
    status: "ready",
    origin: "provisioned",
    spec: {},
    ...over,
  };
}

let attachFn: ReturnType<typeof vi.fn>;
let detachFn: ReturnType<typeof vi.fn>;
let retryFn: ReturnType<typeof vi.fn>;
let testFn: ReturnType<typeof vi.fn>;
let invalidated: unknown[][];

function mockEngines(list: EngineDto[], over: Record<string, unknown> = {}) {
  vi.mocked(enginesApi.useControlPlaneEnginesListEngines).mockReturnValue({
    data: { status: 200, data: list },
    isLoading: false,
    isError: false,
    ...over,
  } as unknown as ReturnType<typeof enginesApi.useControlPlaneEnginesListEngines>);
}

/**
 * Wire one generated mutation hook to a real `useMutation` over `fn`, keeping
 * the caller's own `mutation` options (that is where `onSuccess`/`onError`
 * live).
 */
type MutationHook = (options?: { mutation?: Record<string, unknown> }) => unknown;

function wire(hook: unknown, fn: ReturnType<typeof vi.fn>) {
  vi.mocked(hook as MutationHook).mockImplementation((options) =>
    useMutation({ mutationFn: fn as never, ...options?.mutation }),
  );
}

function renderTab(c: RegistryCatalogDto = catalog(), me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  invalidated = [];
  const original = qc.invalidateQueries.bind(qc);
  vi.spyOn(qc, "invalidateQueries").mockImplementation((filters) => {
    invalidated.push((filters as { queryKey?: unknown[] })?.queryKey ?? []);
    return original(filters);
  });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter>
            <EnginesTab catalog={c} />
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const setup = () => userEvent.setup({ pointerEventsCheck: 0 });

/** The row whose Engine cell names `engineName`. */
function rowFor(engineName: string) {
  return screen.getByRole("row", { name: new RegExp(engineName) });
}

/**
 * Render the tab, press Test once, and hand back the result box — whether the
 * route answered with a 200 body or threw (the documented 400).
 */
async function runTest(result: CatalogTestResultDto | ApiError) {
  if (result instanceof ApiError) testFn.mockRejectedValue(result);
  else testFn.mockResolvedValue({ status: 200, data: result });
  const user = setup();
  renderTab();
  await user.click(screen.getByRole("button", { name: /^Test$/ }));
  return await screen.findByTestId("catalog-test-result");
}

beforeEach(() => {
  vi.clearAllMocks();

  attachFn = vi.fn().mockResolvedValue({ status: 201, data: attachment({ engineId: "e2" }) });
  detachFn = vi.fn().mockResolvedValue({ status: 204, data: undefined });
  retryFn = vi
    .fn()
    .mockResolvedValue({
      status: 200,
      data: { engineId: "e1", engineName: "trino-prod", status: "applied", error: null },
    });
  testFn = vi.fn();

  wire(registry.useControlPlaneCatalogRegistryRegistryAttachCatalog, attachFn);
  wire(registry.useControlPlaneCatalogRegistryDetachCatalog, detachFn);
  wire(registry.useControlPlaneCatalogRegistryRetryCatalogApply, retryFn);
  wire(registry.useControlPlaneCatalogRegistryTestRegistryCatalog, testFn);

  mockEngines([engine(), engine({ id: "e2", name: "sr-eu", type: "starrocks" })]);
});

describe("EnginesTab — the attachment table", () => {
  test("the three apply statuses render distinct badges, and a failed row shows its error as text", () => {
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "applied" }),
          attachment({ engineId: "e2", engineName: "sr-eu", applyStatus: "pending" }),
          attachment({
            engineId: "e3",
            engineName: "sr-us",
            applyStatus: "failed",
            applyError: "FE refused: catalog `sales` already exists with different properties",
          }),
        ],
      }),
    );

    expect(within(rowFor("trino-prod")).getByText("applied")).toBeInTheDocument();
    expect(within(rowFor("sr-eu")).getByText("pending")).toBeInTheDocument();
    expect(within(rowFor("sr-us")).getByText("failed")).toBeInTheDocument();

    // Three distinct words is only half of it: the three must not be the same
    // badge with three captions, or "failed" reads as calmly as "applied".
    const applied = within(rowFor("trino-prod")).getByText("applied").className;
    const pending = within(rowFor("sr-eu")).getByText("pending").className;
    const failed = within(rowFor("sr-us")).getByText("failed").className;
    expect(new Set([applied, pending, failed]).size).toBe(3);

    // The one string that says what to fix, in full, outside the `title`
    // attribute — a tooltip is not something an operator can copy or screenshot.
    expect(
      screen.getByText(/FE refused: catalog `sales` already exists with different properties/),
    ).toBeInTheDocument();
  });

  test("a primary attachment is badged, cannot be detached, and says why; a non-primary one can", () => {
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", isPrimary: true }),
          attachment({ engineId: "e2", engineName: "sr-eu", isPrimary: false }),
        ],
      }),
    );

    const primary = rowFor("trino-prod");
    expect(within(primary).getByText("primary")).toBeInTheDocument();
    expect(within(primary).getByRole("button", { name: /Detach/ })).toBeDisabled();
    // The reason on screen, not only in a disabled attribute.
    expect(
      within(primary).getByText(/primary catalog.*deleting the engine/i),
    ).toBeInTheDocument();

    expect(within(rowFor("sr-eu")).getByRole("button", { name: /Detach/ })).toBeEnabled();
    expect(within(rowFor("sr-eu")).queryByText("primary")).not.toBeInTheDocument();
  });

  test("Retry is offered on failed and pending rows and not on an applied one", () => {
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "applied" }),
          attachment({ engineId: "e2", engineName: "sr-eu", applyStatus: "pending" }),
          attachment({
            engineId: "e3",
            engineName: "sr-us",
            applyStatus: "failed",
            applyError: "boom",
          }),
        ],
      }),
    );

    expect(within(rowFor("sr-eu")).getByRole("button", { name: /Retry/ })).toBeInTheDocument();
    expect(within(rowFor("sr-us")).getByRole("button", { name: /Retry/ })).toBeInTheDocument();
    expect(
      within(rowFor("trino-prod")).queryByRole("button", { name: /Retry/ }),
    ).not.toBeInTheDocument();
  });

  test("an applyStatus egret does not recognise is never badged as applied", () => {
    // `CatalogAttachmentDto.applyStatus` is generated as `string`, not an
    // enum, so this row is representable. The badge must show what arrived,
    // not fall through to the green one — a false green asserting a property
    // nothing measured.
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "applied" }),
          attachment({ engineId: "e2", engineName: "sr-eu", applyStatus: "applying" }),
        ],
      }),
    );
    const unknown = within(rowFor("sr-eu")).getByText("applying");
    expect(unknown).toBeInTheDocument();
    expect(within(rowFor("sr-eu")).queryByText("applied")).not.toBeInTheDocument();
    expect(unknown.className).not.toBe(
      within(rowFor("trino-prod")).getByText("applied").className,
    );
  });

  test("the Applied cell never says a failed row is waiting, and qualifies a stale timestamp", () => {
    renderTab(
      catalog({
        engines: [
          // `set_apply_failed` does not clear `applied_at`, so this row is
          // real: it applied once and later failed.
          attachment({
            engineId: "e1",
            engineName: "was-applied",
            applyStatus: "failed",
            applyError: "FE refused",
            appliedAt: "2026-09-01T00:00:00Z",
          }),
          attachment({
            engineId: "e2",
            engineName: "never-applied",
            applyStatus: "failed",
            applyError: "FE refused",
            appliedAt: null,
          }),
          attachment({
            engineId: "e3",
            engineName: "sr-pending",
            applyStatus: "pending",
            appliedAt: null,
          }),
        ],
      }),
    );

    expect(within(rowFor("was-applied")).getByText(/^last applied /)).toBeInTheDocument();
    expect(within(rowFor("never-applied")).getByText("never applied")).toBeInTheDocument();
    // A failed row saying "waiting since" contradicts its own badge.
    expect(screen.queryAllByText(/waiting since/)).toHaveLength(1);
    expect(within(rowFor("sr-pending")).getByText(/^waiting since /)).toBeInTheDocument();
  });

  test("a failed ENGINE is visible even though its attachment reads applied", () => {
    // The defect Task 8's review found: `applyStatus` answers "did the catalog
    // reach that engine", and a cluster that died afterwards still answers
    // `applied`. An operator scanning the apply column for trouble saw none.
    mockEngines([
      engine({ id: "e1", name: "trino-prod", status: "failed" }),
      engine({ id: "e2", name: "sr-eu", type: "starrocks", status: "running" }),
    ]);
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "applied" }),
          attachment({ engineId: "e2", engineName: "sr-eu", applyStatus: "applied" }),
        ],
      }),
    );

    const broken = rowFor("trino-prod");
    expect(within(broken).getByText("applied")).toBeInTheDocument();
    expect(within(broken).getByText("failed")).toBeInTheDocument();
    // …and the healthy engine is not painted the same way. The colour is the
    // part an operator scans; two words in one variant is the same defect in a
    // smaller costume.
    const healthy = within(rowFor("sr-eu")).getByText("running");
    expect(healthy).toBeInTheDocument();
    expect(within(rowFor("sr-eu")).queryByText("failed")).not.toBeInTheDocument();
    expect(within(broken).getByText("failed").className).not.toBe(healthy.className);
  });

  test("the apply column is not named Status, and the table says what it is not", () => {
    renderTab();
    expect(screen.getByRole("columnheader", { name: "Apply" })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: "Status" })).not.toBeInTheDocument();
    // The column header alone cannot carry this: the sentence is what stops
    // "applied" being read as "this engine is fine".
    expect(screen.getByTestId("catalog-apply-footnote")).toHaveTextContent(
      /not the engine's own health/i,
    );
  });

  test("an attachment whose engine is gone says so rather than reading as healthy", () => {
    // The list was read and this engine was not in it — Task 3's D9 path, an
    // attachment that outlived its engine row.
    mockEngines([engine({ id: "e1", name: "trino-prod", status: "running" })]);
    renderTab(catalog({ engines: [attachment({ engineId: "e9", engineName: "ghost" })] }));
    expect(within(rowFor("ghost")).getByText("engine unknown")).toBeInTheDocument();
  });

  test("an operator who may not read engines is told egret cannot show engine health", () => {
    renderTab(catalog(), READER);
    // Not a silent blank: an absent badge reads as "fine", which is the same
    // false green in a different costume.
    expect(screen.queryByText("running")).not.toBeInTheDocument();
    expect(screen.queryByText("engine unknown")).not.toBeInTheDocument();
    expect(screen.getByTestId("catalog-apply-footnote")).toHaveTextContent("engine:read");
  });

  test("a catalog attached to nothing says so, rather than rendering an empty table", () => {
    renderTab(catalog({ engines: [] }));
    expect(screen.getByText(/not attached to any engine/i)).toBeInTheDocument();
  });
});

describe("EnginesTab — attach, detach, retry", () => {
  test("attach calls the registry route with { id: catalogId, engineId }", async () => {
    const user = setup();
    renderTab();

    await user.click(screen.getByRole("combobox", { name: /Engine to attach/i }));
    await user.click(await screen.findByRole("option", { name: /sr-eu/ }));
    await user.click(screen.getByRole("button", { name: /^Attach$/ }));

    // The whole object, not `id` alone: both fields are strings, so a swap
    // type-checks and would attach the catalog to nothing while returning 404.
    // `mock.calls[0][0]`, not `toHaveBeenCalledWith`: react-query hands the
    // mutation function a second argument of its own.
    await waitFor(() => expect(attachFn).toHaveBeenCalledTimes(1));
    expect(attachFn.mock.calls[0][0]).toEqual({ id: "c-1", engineId: "e2" });
    await waitFor(() => expect(invalidated).toContainEqual(catalogKey("c-1")));
  });

  test("the attach picker offers only engines this catalog is not already on", async () => {
    const user = setup();
    renderTab();

    await user.click(screen.getByRole("combobox", { name: /Engine to attach/i }));
    expect(await screen.findByRole("option", { name: /sr-eu/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /trino-prod/ })).not.toBeInTheDocument();
  });

  test("a 201 whose applyStatus is failed reports the refusal, not a success", async () => {
    attachFn.mockResolvedValue({
      status: 201,
      data: attachment({
        engineId: "e2",
        engineName: "sr-eu",
        applyStatus: "failed",
        applyError: "FE unreachable",
      }),
    });
    const user = setup();
    renderTab();

    await user.click(screen.getByRole("combobox", { name: /Engine to attach/i }));
    await user.click(await screen.findByRole("option", { name: /sr-eu/ }));
    await user.click(screen.getByRole("button", { name: /^Attach$/ }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(vi.mocked(toast.error).mock.calls[0][0]).toMatch(/FE unreachable/);
    expect(toast.success).not.toHaveBeenCalled();
  });

  test("detach calls the registry route with { id: catalogId, engineId } — the opposite order to retry", async () => {
    const user = setup();
    renderTab(
      catalog({ engines: [attachment({ engineId: "e9", engineName: "sr-eu" })] }),
    );

    await user.click(within(rowFor("sr-eu")).getByRole("button", { name: /Detach/ }));
    // The confirm word is the catalog's name, not "delete": this removes the
    // catalog from ONE engine and the dialog must not read like a delete.
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("sales"), "sales");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));

    await waitFor(() => expect(detachFn).toHaveBeenCalledTimes(1));
    expect(detachFn.mock.calls[0][0]).toEqual({ id: "c-1", engineId: "e9" });
    await waitFor(() => expect(invalidated).toContainEqual(catalogKey("c-1")));
  });

  test("the 400 a primary attachment returns is surfaced verbatim, from .message", async () => {
    detachFn.mockRejectedValue(
      new ApiError(400, "sales is trino-prod's primary catalog; delete the engine instead"),
    );
    const user = setup();
    // The server stays the refusal point: the disabled button is a courtesy,
    // so the refusal path is exercised through a row the client thinks is fine.
    renderTab(
      catalog({ engines: [attachment({ engineId: "e9", engineName: "sr-eu" })] }),
    );

    await user.click(within(rowFor("sr-eu")).getByRole("button", { name: /Detach/ }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("sales"), "sales");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(vi.mocked(toast.error).mock.calls[0][0]).toBe(
      "sales is trino-prod's primary catalog; delete the engine instead",
    );
  });

  test("Retry calls the per-attachment route with { id: engineId, cid: catalogId }", async () => {
    const user = setup();
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e7", engineName: "sr-eu", applyStatus: "failed", applyError: "x" }),
        ],
      }),
    );

    await user.click(within(rowFor("sr-eu")).getByRole("button", { name: /Retry/ }));

    // Inverted against attach and detach above, because this route is mounted
    // under the engine. Asserting the object is the only way to catch a swap.
    await waitFor(() => expect(retryFn).toHaveBeenCalledTimes(1));
    expect(retryFn.mock.calls[0][0]).toEqual({ id: "e7", cid: "c-1" });
    await waitFor(() => expect(invalidated).toContainEqual(catalogKey("c-1")));
  });

  test("an operator who may not list engines gets the missing permission, not the word forbidden", () => {
    mockEngines([], { isError: true, error: new ApiError(403, "forbidden"), data: undefined });
    renderTab();

    const blocked = screen.getByTestId("catalog-attach-blocked");
    expect(blocked).toHaveTextContent("engine:read");
    // `ApiError.message` for a 403 is `Error::Forbidden`'s Display — the bare
    // word, naming no permission and offering no way forward.
    expect(blocked.textContent).not.toMatch(/^forbidden\.?$/);
    // And no picker: a Select with nothing in it and an Attach button that can
    // never fire is a control that lies about what the operator can do.
    expect(screen.queryByRole("combobox", { name: /Engine to attach/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Attach$/ })).not.toBeInTheDocument();
  });

  test("a non-403 failure to list engines shows the server's own sentence", () => {
    mockEngines([], {
      isError: true,
      error: new ApiError(500, "the engine service is unavailable"),
      data: undefined,
    });
    renderTab();

    const blocked = screen.getByTestId("catalog-attach-blocked");
    expect(blocked).toHaveTextContent("the engine service is unavailable");
    expect(blocked).not.toHaveTextContent("engine:read");
  });

  test("a platform with no engines at all does not claim they all have this catalog", () => {
    mockEngines([]);
    renderTab(catalog({ engines: [] }));
    expect(screen.getByText("There are no engines to attach this catalog to.")).toBeInTheDocument();
  });

  /**
   * SP3c-1 Task 11, found in a browser on kind — the arm the 403 test above
   * cannot reach.
   *
   * `useControlPlaneEnginesListEngines` is `enabled: canReadEngines`, so for an
   * operator who may attach but may NOT list engines the request is never sent:
   * `isError` stays false and the blocked alert never renders. The picker then
   * fell through to "There are no engines to attach this catalog to." — a claim
   * about the platform, made from an answer about permissions, beside a Select
   * that could never fill. It is the distinction `engineHealth` already draws:
   * "egret cannot know" is not "there are none".
   *
   * The persona is the point: `catalog_registry:update` WITHOUT `engine:read`,
   * which is what puts the attach block on screen at all.
   */
  test("an operator who may attach but may not list engines is told why, not that there are none", () => {
    const ATTACHER: Me = {
      user_id: "u3",
      email: "attacher@example.com",
      platform_roles: [],
      permissions: ["catalog_registry:read", "catalog_registry:update"],
    };
    // The query is DISABLED for this persona, so react-query reports neither
    // data nor an error — the state the 403 test never enters.
    mockEngines([], { isError: false, error: null, data: undefined });
    renderTab(catalog({ engines: [] }), ATTACHER);

    const blocked = screen.getByTestId("catalog-attach-blocked");
    expect(blocked).toHaveTextContent("engine:read");
    expect(
      screen.queryByText("There are no engines to attach this catalog to."),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /Engine to attach/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Attach$/ })).not.toBeInTheDocument();
  });

  test("a successful change drops the Test verdict it has just invalidated", async () => {
    const user = setup();
    testFn.mockResolvedValue({
      status: 200,
      data: {
        ok: false,
        check: "tcp",
        engineName: null,
        target: "db.internal:5432",
        message: "Could not reach db.internal:5432: connection refused.",
      },
    });
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e7", engineName: "sr-eu", applyStatus: "failed", applyError: "x" }),
        ],
      }),
    );

    await user.click(screen.getByRole("button", { name: /^Test$/ }));
    expect(await screen.findByTestId("catalog-test-result")).toHaveTextContent(
      "connection refused",
    );

    // The verdict was measured against an attachment set that no longer
    // exists — the route chooses `query` over `tcp` by looking for an applied
    // Trino attachment — so it must not sit there above a row that just went
    // green.
    await user.click(within(rowFor("sr-eu")).getByRole("button", { name: /Retry/ }));
    await waitFor(() =>
      expect(screen.queryByTestId("catalog-test-result")).not.toBeInTheDocument(),
    );
  });

  test("an operator who may read engines gets a link to the engine", () => {
    renderTab(catalog({ engines: [attachment({ engineId: "e1", engineName: "trino-prod" })] }));
    expect(screen.getByRole("link", { name: "trino-prod" })).toHaveAttribute(
      "href",
      "/control-plane/engines/e1",
    );
  });

  test("an operator who may not read engines gets the name, not a link that would 403", () => {
    // `catalog_registry:read` does not imply `engine:read`, so the engine page
    // would refuse exactly the persona this tab was built for. The name is
    // still shown — a link that 403s is worse than a plain identifier. Two
    // tests, not one: a second `render` in the same test leaves the first
    // one's DOM in the document, so the negative assertion would be reading
    // the positive case's markup.
    renderTab(
      catalog({ engines: [attachment({ engineId: "e1", engineName: "trino-prod" })] }),
      READER,
    );
    expect(screen.getByText("trino-prod")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "trino-prod" })).not.toBeInTheDocument();
  });

  test("an operator who may only read gets no attach, detach or retry controls", () => {
    renderTab(
      catalog({
        engines: [attachment({ engineId: "e1", applyStatus: "failed", applyError: "x" })],
      }),
      READER,
    );

    expect(screen.queryByRole("button", { name: /^Attach$/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Detach/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
    // Test is a read — `catalog_registry:read` — so it stays.
    expect(screen.getByRole("button", { name: /^Test$/ })).toBeInTheDocument();
  });
});

describe("EnginesTab — Test names the check that ran (D4)", () => {
  test("`query` names the statement and the engine it ran on, from the client's own line", async () => {
    const box = await runTest({
      ok: true,
      // The server's real success sentence quotes the statement and the engine
      // itself. Deliberately NOT used here: an assertion the mocked `message`
      // already satisfies passes whether or not the renderer names the check,
      // which is the one thing this branch exists to do. This message names
      // neither, so only the client's line can satisfy the assertions below.
      check: "query",
      engineName: "trino-prod",
      target: null,
      message: "the catalog answered — credentials included.",
    });
    expect(box).toHaveTextContent("SHOW SCHEMAS FROM sales");
    expect(box).toHaveTextContent("trino-prod");
    expect(box).toHaveTextContent("credentials included");
    expect(box).toHaveAttribute("data-tone", "ok");
  });

  test("`tcp` says in the sentence that it proves nothing about credentials, and names the target", async () => {
    const box = await runTest({
      ok: true,
      check: "tcp",
      engineName: null,
      target: "db.internal:5432",
      // Names neither the host nor the port — the server's real sentence does,
      // and while it did, `toHaveTextContent("db.internal:5432")` was satisfied
      // by the SERVER half whatever the renderer printed. Same shape as the
      // `query` fixture one describe block up; the fix was applied there and
      // not carried here (SP3c review F2).
      message: "the port answered.",
    });
    expect(box).toHaveTextContent("db.internal:5432");
    expect(box).toHaveTextContent(/does not verify credentials/i);
    expect(box).toHaveTextContent("the port answered");
    // The caveat is text, not a tooltip: a `title` a screenshot loses.
    expect(box.querySelector("[title]")).toBeNull();
    // …and it is NOT the same line `query` prints.
    expect(box).not.toHaveTextContent("SHOW SCHEMAS FROM");
  });

  test("a refused `tcp` check does not claim egret opened the connection", async () => {
    const box = await runTest({
      ok: false,
      check: "tcp",
      engineName: null,
      target: "db.internal:5432",
      message: "the connection was refused.",
    });
    expect(box).toHaveAttribute("data-tone", "bad");
    // The failure said "Could not reach …" while the line above it said egret
    // "opened and closed a TCP connection" to the same address.
    expect(box).not.toHaveTextContent(/opened and closed/i);
    expect(box).toHaveTextContent(/could not open a TCP connection/i);
    // Still names what it dialled, and still carries the caveat: the
    // limitation is a property of the check, not of its outcome.
    expect(box).toHaveTextContent("db.internal:5432");
    expect(box).toHaveTextContent(/verify credentials/i);
  });

  test("`unsupported` renders neutral rather than red, although ok is false", async () => {
    const box = await runTest({
      ok: false,
      check: "unsupported",
      engineName: null,
      target: null,
      message:
        "A catalog of kind `tpch` has no network endpoint egret can reach on its own, so there was nothing to check.",
    });
    // `ok:false` is deliberate on this branch — nothing was measured — and a
    // tpch catalog with nothing attached is not broken. Colouring it as a
    // failure teaches operators to ignore the colour.
    expect(box).toHaveAttribute("data-tone", "neutral");
    expect(box).toHaveTextContent(/no test/i);
    expect(box).toHaveTextContent("tpch");
    expect(box).not.toHaveTextContent("SHOW SCHEMAS FROM");
    expect(box).not.toHaveTextContent(/does not verify credentials/i);
  });

  test("a failed `query` is red and still names the statement", async () => {
    const box = await runTest({
      ok: false,
      check: "query",
      engineName: "trino-prod",
      target: null,
      message: "Catalog 'sales' does not exist",
    });
    expect(box).toHaveAttribute("data-tone", "bad");
    expect(box).toHaveTextContent("SHOW SCHEMAS FROM");
    expect(box).toHaveTextContent("Catalog 'sales' does not exist");
    // …and does NOT carry the success path's credentials claim: this very
    // fixture is a statement that failed before any credential was used.
    expect(box).not.toHaveTextContent(/which exercises the catalog's credentials/i);
    expect(box).toHaveTextContent(/nothing was shown about the catalog's credentials/i);
  });

  test("the 400 a stored row the validator now refuses returns is rendered, not swallowed", async () => {
    const box = await runTest(
      new ApiError(400, "invalid hive configuration: uri must name a host"),
    );
    expect(box).toHaveAttribute("data-tone", "bad");
    expect(box).toHaveTextContent("invalid hive configuration: uri must name a host");
  });
});

/**
 * Whole-branch review, F1, narrowed by SP3c-2 Task 3. `EngineDetail` disables
 * Retry and Detach on EVERY row of a Trino engine while any of that engine's
 * OTHER catalogs has a stored configuration egret cannot parse — a SIBLING's
 * `configError`, which lives on that sibling's own `RegistryCatalogDto` and is
 * structurally invisible from here: this page renders one catalog across many
 * engines, not one engine's whole attachment set. (THIS catalog's own
 * `configError` is no longer in that boat — it is `catalog.configError` on
 * the very prop this page already holds, so `retryBlockedReason` uses it
 * directly rather than passing `null`; see `attachmentUi.tsx`'s module doc.)
 * So the sibling case is still an explicit `null` plus one sentence saying
 * what the `null` costs, and these tests pin the sentence to the engines it
 * can actually be true of.
 */
describe("EnginesTab — the refusal scope this page cannot predict", () => {
  test("names the Trino engines a Retry can be refused for, and leaves the button enabled", () => {
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "failed", applyError: "boom" }),
          attachment({ engineId: "e2", engineName: "sr-eu", engineType: "starrocks" }),
        ],
      }),
    );

    const note = screen.getByTestId("catalog-retry-scope-note");
    expect(note).toHaveTextContent("trino-prod");
    // The StarRocks engine must NOT be listed: its apply really is one
    // statement about one catalog, so no sibling can refuse it. Naming it would
    // be the same over-claim in the opposite direction.
    expect(note).not.toHaveTextContent("sr-eu");
    expect(note).toHaveTextContent(/different/i);
    // And the server stays the refusal point: a disabled button with an
    // invented reason would be worse than an enabled one that explains itself.
    expect(within(rowFor("trino-prod")).getByRole("button", { name: /Retry/ })).toBeEnabled();
  });

  test("says nothing when every attached engine is StarRocks", () => {
    renderTab(
      catalog({
        engines: [
          attachment({ engineId: "e2", engineName: "sr-eu", engineType: "starrocks" }),
          attachment({ engineId: "e3", engineName: "sr-us", engineType: "starrocks" }),
        ],
      }),
    );
    expect(screen.queryByTestId("catalog-retry-scope-note")).not.toBeInTheDocument();
  });

  test("treats an engine type egret does not recognise as the cautious case", () => {
    renderTab(catalog({ engines: [attachment({ engineId: "e9", engineName: "duck-1", engineType: "duckdb" })] }));
    expect(screen.getByTestId("catalog-retry-scope-note")).toHaveTextContent("duck-1");
  });

  test("is not shown to an operator who is offered no Retry or Detach at all", () => {
    renderTab(catalog(), READER);
    expect(screen.queryByTestId("catalog-retry-scope-note")).not.toBeInTheDocument();
  });
});

/**
 * SP3c-2 Task 3. THIS is the half of the refusal scope the page CAN predict
 * now: `configError` moved onto `RegistryCatalogDto`, which is the `catalog`
 * prop this page already holds, so a broken stored configuration blocks
 * Retry on every row — Trino or StarRocks alike, because rendering ONE
 * catalog fails the same way regardless of the engine — without the `null`
 * the sibling case above still needs.
 */
describe("EnginesTab — this catalog's own configError", () => {
  test("blocks Retry on every attached row, Trino and StarRocks alike, with the reason on screen", () => {
    renderTab(
      catalog({
        configError: "missing field `dialect`",
        engines: [
          attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "failed", applyError: "boom" }),
          attachment({
            engineId: "e2",
            engineName: "sr-eu",
            engineType: "starrocks",
            applyStatus: "failed",
            applyError: "boom",
          }),
        ],
      }),
    );

    for (const name of ["trino-prod", "sr-eu"]) {
      const row = rowFor(name);
      expect(within(row).getByRole("button", { name: /Retry/ })).toBeDisabled();
      expect(
        within(row).getByText(/cannot parse this catalog's stored configuration/),
      ).toBeInTheDocument();
    }
    // Detach is unaffected: a broken catalog can still be removed from an
    // engine (it needs no render to drop), so this is Retry-only.
    expect(within(rowFor("trino-prod")).getByRole("button", { name: /Detach/ })).toBeEnabled();
  });

  test("a healthy catalog's Retry names no configError reason", () => {
    renderTab(
      catalog({
        engines: [attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "failed" })],
      }),
    );

    expect(
      within(rowFor("trino-prod")).queryByText(/cannot parse this catalog's stored configuration/),
    ).toBeNull();
  });
});

/**
 * Re-review, N1. The scope note's closing sentence used to read "A refused
 * Retry changes nothing", unconditionally. That is true only of the refusal the
 * paragraph is about — a Build refusal, which never reaches the engine and
 * which F2 stopped flagging. A Retry the ENGINE rejects is `ApplyError::Rejected`
 * and still writes `failed` onto this row, deliberately. Understating the risk
 * in the reassuring direction is the direction this branch spent eleven tasks
 * correcting.
 */
test("the scope note's reassurance is scoped to the refusal it is about", () => {
  renderTab(
    catalog({
      engines: [attachment({ engineId: "e1", engineName: "trino-prod", applyStatus: "pending" })],
    }),
  );
  const note = screen.getByTestId("catalog-retry-scope-note");
  // The reassurance is qualified…
  expect(note).toHaveTextContent(/refused for that reason changes nothing/i);
  // …and the other outcome is named rather than left to be inferred.
  expect(note).toHaveTextContent(/engine itself rejects/i);
  expect(note).toHaveTextContent(/failed/);
  // The unqualified claim must not be recoverable from the text.
  expect(note).not.toHaveTextContent(/A refused Retry changes nothing/i);
});
