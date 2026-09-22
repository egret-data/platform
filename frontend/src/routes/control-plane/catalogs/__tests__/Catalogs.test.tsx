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
import type { RegistryCatalogDto } from "@/api/generated/model";
import Catalogs, {
  attachmentWarning,
  catalogsQueryKey,
  isEgretWarehouse,
} from "@/routes/control-plane/catalogs/Catalogs";

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

function renderPage(entry = "/control-plane/catalogs") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[entry]}>
        <Catalogs />
      </MemoryRouter>
    </QueryClientProvider>,
  );
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
  vi.mocked(registry.useControlPlaneCatalogRegistryDeleteRegistryCatalog).mockReturnValue({
    mutate: registryDelete,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryDeleteRegistryCatalog>);
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
 */
describe("attachmentWarning", () => {
  const pending = (over: Record<string, unknown> = {}) => ({
    engineId: "e1",
    engineName: "sr-new",
    engineType: "starrocks",
    applyStatus: "pending",
    appliedAt: null,
    isPrimary: false,
    ...over,
  });

  test("failed is unambiguous and names the engine", () => {
    const w = attachmentWarning(
      catalog({ engines: [pending({ applyStatus: "failed", engineName: "sr-a" })] }),
      NOW,
    );
    expect(w).toEqual({ level: "failed", text: "Failed to apply on sr-a" });
  });

  test("a pending attachment on a catalog updated a minute ago is not a warning", () => {
    const updatedAt = new Date(NOW - 60_000).toISOString();
    expect(attachmentWarning(catalog({ engines: [pending()], updatedAt }), NOW)).toBeNull();
  });

  test("a pending attachment on a catalog updated an hour ago is stalled", () => {
    const updatedAt = new Date(NOW - 60 * 60_000).toISOString();
    const w = attachmentWarning(catalog({ engines: [pending()], updatedAt }), NOW);
    expect(w?.level).toBe("stalled");
    expect(w?.text).toContain("sr-new");
  });

  test("failed outranks a stalled pending on the same catalog", () => {
    const updatedAt = new Date(NOW - 60 * 60_000).toISOString();
    const w = attachmentWarning(
      catalog({
        engines: [pending(), pending({ engineId: "e2", engineName: "trino-b", applyStatus: "failed" })],
        updatedAt,
      }),
      NOW,
    );
    expect(w?.level).toBe("failed");
  });

  test("an old pending that has already applied once is not stalled", () => {
    const updatedAt = new Date(NOW - 60 * 60_000).toISOString();
    const engines = [pending({ appliedAt: "2026-09-22T09:00:00Z" })];
    expect(attachmentWarning(catalog({ engines, updatedAt }), NOW)).toBeNull();
  });

  test("an applied attachment is never a warning, however old", () => {
    const updatedAt = new Date(NOW - 60 * 60_000).toISOString();
    const engines = [pending({ applyStatus: "applied", appliedAt: null })];
    expect(attachmentWarning(catalog({ engines, updatedAt }), NOW)).toBeNull();
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
