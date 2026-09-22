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
    warehouseId: "wh-1",
    status: "ready",
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
