import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The list reads the GENERATED registry client, so that is what is mocked here
 * — the same reason `ServiceAccountCreate.invalidation.test.tsx` mocks the
 * generated module rather than a hand-written wrapper. `@/api/controlPlane` is
 * mocked for `deleteWarehouse`, which is the OTHER endpoint delete routes to
 * (an egret warehouse catalog is deleted with its warehouse — the registry
 * refuses it unconditionally with 409 `warehouse_catalog`).
 */
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
import type { Me } from "@/api/hooks";
import type { RegistryCatalogDto } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import Catalogs, {
  attachmentWarning,
  catalogsQueryKey,
  isEgretWarehouse,
} from "@/routes/control-plane/catalogs/Catalogs";
import { warehousesQueryKey } from "@/routes/control-plane/queryKeys";

const NOW = new Date("2026-09-22T12:00:00Z").getTime();

function catalog(over: Partial<RegistryCatalogDto> = {}): RegistryCatalogDto {
  return {
    id: "c1",
    name: "analytics",
    kind: "iceberg",
    config: {} as RegistryCatalogDto["config"],
    advanced: {} as RegistryCatalogDto["advanced"],
    warehouseId: "wh-1",
    engines: [
      {
        engineId: "e1",
        engineName: "trino-prod",
        engineType: "trino",
        applyStatus: "applied",
        appliedAt: "2026-09-22T11:00:00Z",
        isPrimary: true,
        createdAt: "2026-09-22T10:30:00Z",
      },
    ],
    workspaceIds: ["ws1"],
    createdAt: "2026-09-22T10:00:00Z",
    updatedAt: "2026-09-22T11:00:00Z",
    ...over,
  };
}

const JDBC = catalog({
  id: "c2",
  name: "crm",
  kind: "jdbc",
  warehouseId: null,
  engines: [],
  workspaceIds: [],
});

/** The generated list hook's return value, narrowed to what the page reads. */
function listResult(rows: RegistryCatalogDto[], over: Record<string, unknown> = {}) {
  return {
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    ...over,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>;
}

let registryDelete: ReturnType<typeof vi.fn>;
/** The options the page handed the generated delete hook, so a test can drive
 * the `onSuccess` the page attached — the mocked hook never runs it itself. */
let registryDeleteOptions:
  | Parameters<typeof registry.useControlPlaneCatalogRegistryDeleteRegistryCatalog>[0]
  | undefined;

/**
 * `platform_admin` by default, so every test written before the page gated its
 * controls still measures what it was written to measure. The gating tests pass
 * their own persona.
 */
const ADMIN: Me = {
  user_id: "u1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};
/** The persona D3 invented and Task 11 proved lands here first. */
const READER: Me = {
  user_id: "u2",
  email: "reader@example.com",
  platform_roles: [],
  permissions: ["catalog_registry:read"],
};

function renderPage(entry = "/control-plane/catalogs", me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  const utils = render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[entry]}>
          <Catalogs />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
  return { ...utils, qc, invalidate };
}

/** Opens the row's Delete dialog and types the confirmation word. */
async function confirmDelete(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole("button", { name: `Delete ${name}` }));
  await user.type(await screen.findByPlaceholderText("delete"), "delete");
  await user.click(screen.getByRole("button", { name: /^Delete$/ }));
}

beforeEach(() => {
  vi.clearAllMocks();
  registryDelete = vi.fn();
  vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
    data: [{ id: "ws1", slug: "eng", name: "Engineering" }],
  } as ReturnType<typeof hooks.useMyWorkspaces>);
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
    listResult([]),
  );
  registryDeleteOptions = undefined;
  vi.mocked(registry.useControlPlaneCatalogRegistryDeleteRegistryCatalog).mockImplementation(
    (options) => {
      registryDeleteOptions = options;
      return { mutate: registryDelete } as unknown as ReturnType<
        typeof registry.useControlPlaneCatalogRegistryDeleteRegistryCatalog
      >;
    },
  );
  vi.mocked(api.deleteWarehouse).mockResolvedValue(undefined);
});

describe("Catalogs list", () => {
  test("renders a catalog with its kind badge, engine count and workspace name", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog()]),
    );
    renderPage();

    expect(screen.getByRole("heading", { name: "Catalogs" })).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "analytics" })).toHaveAttribute(
      "href",
      "/control-plane/catalogs/c1",
    );
    // `warehouseId` is the whole egret-warehouse predicate (migration 0147's
    // CHECK makes it exactly equivalent to iceberg + metastore type `egret`),
    // so the kind reads as the warehouse it is, not the raw `iceberg`.
    expect(screen.getByText("egret warehouse")).toBeInTheDocument();
    expect(screen.queryByText("iceberg")).not.toBeInTheDocument();
    expect(screen.getByText("trino-prod")).toBeInTheDocument();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
  });

  test("a jdbc catalog shows its raw kind", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([JDBC]),
    );
    renderPage();
    expect(await screen.findByText("jdbc")).toBeInTheDocument();
    expect(screen.queryByText("egret warehouse")).not.toBeInTheDocument();
  });

  test("?workspace= scopes the query server-side and offers a clear link", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog()]),
    );
    renderPage("/control-plane/catalogs?workspace=ws1");

    // Unlike the warehouse list, the filter is the ROUTE's — the page must hand
    // it to the hook, not filter the rows it got back.
    expect(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).toHaveBeenCalledWith(
      { workspaceId: "ws1" },
      expect.anything(),
    );
    // The banner names the workspace, not its raw id — scoped to the banner
    // because the row's Workspaces cell renders the same name.
    const banner = (await screen.findByText(/Filtered to workspace/i)).closest("div")!;
    expect(within(banner).getByText("Engineering")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Show all workspaces/i })).toHaveAttribute(
      "href",
      "/control-plane/catalogs",
    );
  });

  test("no ?workspace= passes no params at all", () => {
    renderPage();
    expect(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).toHaveBeenCalledWith(
      undefined,
      expect.anything(),
    );
    expect(screen.queryByText(/Filtered to workspace/i)).not.toBeInTheDocument();
  });

  test("the empty state offers New catalog", async () => {
    renderPage();
    expect(await screen.findByText(/No catalogs yet/i)).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /New catalog/i })[0]).toHaveAttribute(
      "href",
      "/control-plane/catalogs/new",
    );
  });

  test("a failed attachment surfaces as a row warning", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([
        catalog({
          engines: [
            {
              engineId: "e1",
              engineName: "trino-prod",
              engineType: "trino",
              applyStatus: "failed",
              appliedAt: null,
              isPrimary: false,
              createdAt: "2026-09-22T10:30:00Z",
            },
          ],
        }),
      ]),
    );
    renderPage();
    expect(await screen.findByText(/Failed to apply on trino-prod/)).toBeInTheDocument();
  });
});

/**
 * The warning rule is the only non-obvious logic on the page, so it is pure and
 * tested without rendering. `pending` is NORMAL for the first minutes of a
 * StarRocks engine's life (SP3b writes it deliberately), so it is a warning
 * only once it has outlived the bootstrap that clears it.
 *
 * The clock is the ATTACHMENT's own `createdAt`, not the catalog's `updatedAt`
 * (fix round 1, F2). The two are held apart on purpose in the first two cases
 * below — each sets one clock to a value that would give the WRONG answer if
 * the rule read it — so neither timestamp can stand in for the other and still
 * pass.
 */
describe("attachmentWarning", () => {
  const pending = (over: Record<string, unknown> = {}) => ({
    engineId: "e1",
    engineName: "sr-new",
    engineType: "starrocks",
    applyStatus: "pending",
    appliedAt: null,
    isPrimary: false,
    createdAt: new Date(NOW).toISOString(),
    ...over,
  });
  const ago = (ms: number) => new Date(NOW - ms).toISOString();
  const MONTH = 30 * 24 * 60 * 60_000;

  test("failed is unambiguous and names the engine", () => {
    const w = attachmentWarning(
      catalog({ engines: [pending({ applyStatus: "failed", engineName: "sr-a" })] }),
      NOW,
    );
    expect(w).toEqual({ level: "failed", text: "Failed to apply on sr-a" });
  });

  // F2, and the reason the anchor moved. Attaching an EXISTING catalog to a
  // fresh StarRocks engine inserts an attachment row and never touches
  // `registry.catalogs.updated_at` (`store.rs` insert_attachment_row), so the
  // catalog's clock is already a month old at the instant the wait begins.
  // Read off the catalog, this row cried "Still pending on sr-new" from the
  // very first render — throughout exactly the 5-10 minute bootstrap the grace
  // exists to cover.
  test("a pending attachment a minute old is not a warning, however old the catalog row is", () => {
    const engines = [pending({ createdAt: ago(60_000) })];
    expect(attachmentWarning(catalog({ engines, updatedAt: ago(MONTH) }), NOW)).toBeNull();
  });

  // The other direction, so the fix cannot be "ignore time entirely": a catalog
  // edited a second ago (which always re-applies — `update_catalog` calls
  // `propagate`) whose attachment has nonetheless been waiting an hour is
  // stalled. The catalog clock would have hidden this one.
  test("a pending attachment an hour old is stalled, however recently the catalog row changed", () => {
    const engines = [pending({ createdAt: ago(60 * 60_000) })];
    const w = attachmentWarning(catalog({ engines, updatedAt: ago(1_000) }), NOW);
    expect(w?.level).toBe("stalled");
    expect(w?.text).toContain("sr-new");
  });

  test("a pending attachment still inside the 15-minute grace is not a warning", () => {
    const engines = [pending({ createdAt: ago(14 * 60_000) })];
    expect(attachmentWarning(catalog({ engines }), NOW)).toBeNull();
  });

  test("failed outranks a stalled pending on the same catalog", () => {
    const w = attachmentWarning(
      catalog({
        engines: [
          pending({ createdAt: ago(60 * 60_000) }),
          pending({ engineId: "e2", engineName: "trino-b", applyStatus: "failed" }),
        ],
      }),
      NOW,
    );
    expect(w?.level).toBe("failed");
  });

  test("an old pending that has already applied once is not stalled", () => {
    const engines = [pending({ createdAt: ago(60 * 60_000), appliedAt: "2026-09-22T09:00:00Z" })];
    expect(attachmentWarning(catalog({ engines }), NOW)).toBeNull();
  });

  test("an applied attachment is never a warning, however old", () => {
    const engines = [pending({ createdAt: ago(MONTH), applyStatus: "applied", appliedAt: null })];
    expect(attachmentWarning(catalog({ engines }), NOW)).toBeNull();
  });
});

describe("isEgretWarehouse", () => {
  test("is exactly `warehouseId != null`, not a config sniff", () => {
    expect(isEgretWarehouse(catalog())).toBe(true);
    expect(isEgretWarehouse(catalog({ warehouseId: null }))).toBe(false);
    expect(isEgretWarehouse(catalog({ warehouseId: undefined }))).toBe(false);
    // An iceberg catalog with no warehouse is an EXTERNAL metastore, and the
    // registry's CHECK guarantees it carries no `warehouse_id`.
    expect(isEgretWarehouse(catalog({ kind: "iceberg", warehouseId: null }))).toBe(false);
  });
});

/**
 * The delete branch is the one place a wrong guess is destructive-adjacent: the
 * registry refuses an egret warehouse catalog with a 409 the user cannot act
 * on, so that row must go to `deleteWarehouse`. BOTH directions are asserted —
 * a test that only checks the call that happened passes just as well when both
 * fire.
 */
describe("delete routes by kind", () => {
  test("an egret warehouse row deletes the warehouse and never the registry catalog", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog()]),
    );
    const user = userEvent.setup();
    renderPage();
    await confirmDelete(user, "analytics");

    // react-query passes its own context as a second argument to the mutationFn.
    await waitFor(() => expect(api.deleteWarehouse).toHaveBeenCalledWith("wh-1", expect.anything()));
    expect(registryDelete).not.toHaveBeenCalled();
  });

  // F6: an egret-warehouse row is deleted through the very endpoint the
  // Warehouses list calls, so refreshing only this page's cache leaves the
  // deleted warehouse listed on the other section for the 30 seconds of
  // `staleTime` — reachable precisely because D5 keeps both sections live.
  test("an egret warehouse delete refreshes the Warehouses list too", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog()]),
    );
    const user = userEvent.setup();
    const { invalidate } = renderPage();
    await confirmDelete(user, "analytics");

    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({ queryKey: warehousesQueryKey }),
    );
    expect(invalidate).toHaveBeenCalledWith({ queryKey: catalogsQueryKey });
  });

  // The registry branch reaches the same two caches: a jdbc catalog has no
  // warehouse row of its own, but the page must not know two different
  // invalidation rules for one button. Driven through the `onSuccess` the page
  // attached, because the generated hook is mocked and never runs it.
  test("a registry delete refreshes both lists", () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([JDBC]),
    );
    const { invalidate } = renderPage();
    expect(registryDeleteOptions?.mutation?.onSuccess).toBeTypeOf("function");
    registryDeleteOptions!.mutation!.onSuccess!(
      undefined as never,
      { id: "c2" },
      undefined as never,
      undefined as never,
    );
    expect(invalidate).toHaveBeenCalledWith({ queryKey: catalogsQueryKey });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: warehousesQueryKey });
  });

  test("a jdbc row deletes through the registry and never the warehouse endpoint", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([JDBC]),
    );
    const user = userEvent.setup();
    renderPage();
    await confirmDelete(user, "crm");

    await waitFor(() => expect(registryDelete).toHaveBeenCalledWith({ id: "c2" }));
    expect(api.deleteWarehouse).not.toHaveBeenCalled();
  });
});

/**
 * Fix round 1, the two minors.
 *
 * The delete dialog used to explain that the server would refuse and then leave
 * the Delete button enabled, so the user typed the confirmation word and
 * collected the error they had just been warned about; and a workspace the
 * operator is not a member of rendered as eight characters of a UUID, which
 * reads like data and is not.
 */
describe("what the page tells the operator it cannot do", () => {
  const REFERENCED = catalog({
    id: "c3",
    name: "crm-shared",
    kind: "jdbc",
    warehouseId: null,
    engines: [
      {
        engineId: "e1",
        engineName: "trino-prod",
        engineType: "trino",
        applyStatus: "applied",
        appliedAt: "2026-09-22T11:00:00Z",
        isPrimary: false,
        createdAt: "2026-09-22T10:30:00Z",
      },
    ],
    workspaceIds: ["ws1"],
  });

  test("a still-referenced catalog names its references and withholds Delete", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([REFERENCED]),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: "Delete crm-shared" }));
    // The 409's own reasons, read off the row and said BEFORE the user commits.
    expect(await screen.findByText("Attached to engine trino-prod")).toBeInTheDocument();
    expect(screen.getByText("Assigned to workspace Engineering")).toBeInTheDocument();

    // Asserted as BEHAVIOUR, not as `toBeDisabled`: type the word, press the
    // button, and show that nothing was sent. A disabled-attribute assertion
    // can be satisfied by an ancestor the test did not mean to involve.
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    expect(registryDelete).not.toHaveBeenCalled();
    expect(api.deleteWarehouse).not.toHaveBeenCalled();
  });

  test("an unreferenced catalog can still be deleted", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([JDBC]),
    );
    const user = userEvent.setup();
    renderPage();
    await confirmDelete(user, "crm");
    await waitFor(() => expect(registryDelete).toHaveBeenCalledWith({ id: "c2" }));
  });

  test("a workspace the operator cannot see is said to be hidden, not shown as a UUID", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog({ workspaceIds: ["ws1", "3f2a91c4-0000-4000-8000-000000000001"] })]),
    );
    renderPage();
    expect(await screen.findByText(/Engineering, 1 not visible to you/)).toBeInTheDocument();
    expect(screen.queryByText(/3f2a91c4/)).not.toBeInTheDocument();
  });

  test("the ?workspace= banner labels an unresolvable id as an id", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([]),
    );
    renderPage("/control-plane/catalogs?workspace=3f2a91c4-0000-4000-8000-000000000001");
    const banner = (await screen.findByText(/Filtered to workspace id/i)).closest("div")!;
    expect(within(banner).getByText("3f2a91c4-0000-4000-8000-000000000001")).toBeInTheDocument();
  });
});

/**
 * `catalogsQueryKey` is what the page invalidates after a delete, and the list
 * it must refresh is the GENERATED hook's query — whose key the generated
 * builder owns. A hand-written key that looked plausible (`["control-plane",
 * "catalogs"]`) would match nothing and leave the deleted row on screen, with
 * every test still green. This pins the literal to the builder's own output,
 * through `importActual` because this file mocks that module.
 */
describe("catalogsQueryKey", () => {
  test("is the generated list query's key prefix, for every params variant", async () => {
    const actual = await vi.importActual<typeof registry>(
      "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry",
    );
    expect(actual.getControlPlaneCatalogRegistryListRegistryCatalogsQueryKey()).toEqual([
      ...catalogsQueryKey,
    ]);
    expect(
      actual.getControlPlaneCatalogRegistryListRegistryCatalogsQueryKey({ workspaceId: "ws1" })[0],
    ).toBe(catalogsQueryKey[0]);
  });
});

/**
 * Whole-branch review, F3. This is the page a `catalog_registry:read`-only
 * account LANDS on (Task 11 §5.8), and it was the only catalog screen with no
 * permission gate: `New catalog` linked to a route `App.tsx` gates on
 * `catalog_registry:create`, so it could only produce a full-page Forbidden,
 * and the per-row Delete toasted the bare word `forbidden`.
 *
 * The four sibling screens all hide-with-a-reason, so this one does too.
 */
describe("the list gates its own controls", () => {
  test("a catalog_registry:read-only operator is offered neither New catalog nor Delete", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog(), JDBC]),
    );
    renderPage("/control-plane/catalogs", READER);

    // The rows themselves are still there — this is a gate on the actions, not
    // on the list.
    expect(await screen.findByRole("link", { name: "analytics" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /New catalog/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete analytics" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete crm" })).not.toBeInTheDocument();

    // …and it says why, naming each key. A hidden control with no explanation
    // is the shape Task 7's F3, Task 8's F1 and Task 9's F-4 each removed.
    const note = screen.getByTestId("catalogs-readonly");
    expect(note).toHaveTextContent("catalog_registry:create");
    expect(note).toHaveTextContent("catalog_registry:delete");
    expect(note).toHaveTextContent("warehouse:delete");
  });

  test("the two Delete endpoints are gated separately, because they are two endpoints", async () => {
    // A registry catalog goes to `DELETE /catalogs/{id}`; an egret warehouse
    // one goes to `DELETE /warehouses/{id}`. Holding one key must offer exactly
    // the rows it can act on.
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog(), JDBC]),
    );
    renderPage("/control-plane/catalogs", {
      ...READER,
      permissions: ["catalog_registry:read", "catalog_registry:delete"],
    });

    expect(await screen.findByRole("button", { name: "Delete crm" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete analytics" })).not.toBeInTheDocument();
    const note = screen.getByTestId("catalogs-readonly");
    expect(note).toHaveTextContent("warehouse:delete");
    expect(note).not.toHaveTextContent("catalog_registry:delete");
  });

  test("names only the permissions that would have drawn something on THIS list", async () => {
    // No egret warehouse row, so `warehouse:delete` explains a button that was
    // never going to exist — saying it would be noise about a permission the
    // operator does not need.
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([JDBC]),
    );
    renderPage("/control-plane/catalogs", READER);

    const note = await screen.findByTestId("catalogs-readonly");
    expect(note).toHaveTextContent("catalog_registry:delete");
    expect(note).not.toHaveTextContent("warehouse:delete");
  });

  test("the empty state explains the missing create key instead of offering a dead button", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([]),
    );
    renderPage("/control-plane/catalogs", READER);

    expect(await screen.findByText("No catalogs yet")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /New catalog/i })).not.toBeInTheDocument();
    expect(screen.getByText(/catalog_registry:create/)).toBeInTheDocument();
  });

  test("a platform_admin keeps every control and is told nothing is hidden", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog(), JDBC]),
    );
    renderPage();

    expect(await screen.findByRole("button", { name: "Delete analytics" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete crm" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /New catalog/i })[0]).toBeInTheDocument();
    // `me.permissions` does not enumerate what platform_admin implies, so a
    // gate that read only that list would hide everything from the one account
    // that can do it all.
    expect(screen.queryByTestId("catalogs-readonly")).not.toBeInTheDocument();
  });

  test("an operator holding the keys explicitly, without platform_admin, keeps them too", async () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue(
      listResult([catalog(), JDBC]),
    );
    renderPage("/control-plane/catalogs", {
      ...READER,
      permissions: [
        "catalog_registry:read",
        "catalog_registry:create",
        "catalog_registry:delete",
        "warehouse:delete",
      ],
    });

    expect(await screen.findByRole("button", { name: "Delete analytics" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete crm" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /New catalog/i })[0]).toBeInTheDocument();
    expect(screen.queryByTestId("catalogs-readonly")).not.toBeInTheDocument();
  });
});
