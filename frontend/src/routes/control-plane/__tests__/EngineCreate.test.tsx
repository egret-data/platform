import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import EngineCreate from "@/routes/control-plane/EngineCreate";
import * as api from "@/api/controlPlane";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { ApiError } from "@/api/client";
import { warehousesQueryKey } from "@/routes/control-plane/queryKeys";
import type { CreateEngineResponse, RegistryCatalogDto } from "@/api/generated/model";

vi.mock("@/api/controlPlane");
// PARTIAL: the query-key builders stay real, so nothing here pins a literal
// against itself.
vi.mock(
  "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry",
  async (importOriginal) => ({
    ...(await importOriginal<typeof registry>()),
    useControlPlaneCatalogRegistryListRegistryCatalogs: vi.fn(),
  }),
);

const WAREHOUSE: api.Warehouse = {
  id: "w1",
  // NOT the catalog's name. A fixture that calls both "demo" lets an assertion
  // about one of them be satisfied by the other (Task 8's lesson).
  name: "demo-warehouse",
  workspaceId: "ws1",
  storageConnId: "sc1",
  lakekeeperWarehouseId: "lk1",
} as unknown as api.Warehouse;

const CAT_ID = "11111111-1111-4111-8111-111111111111";
const JDBC_ID = "22222222-2222-4222-8222-222222222222";
const SECOND_ID = "33333333-3333-4333-8333-333333333333";

function catalog(over: Partial<RegistryCatalogDto>): RegistryCatalogDto {
  return {
    id: CAT_ID,
    name: "demo-catalog",
    kind: "iceberg_egret",
    config: {},
    advanced: {},
    warehouseId: WAREHOUSE.id,
    engines: [],
    workspaceIds: [],
    createdAt: "2026-09-22T10:00:00Z",
    updatedAt: "2026-09-22T10:00:00Z",
    ...over,
  } as RegistryCatalogDto;
}

const EGRET = catalog({});
const JDBC = catalog({ id: JDBC_ID, name: "sales-pg", kind: "jdbc", warehouseId: null });
const SECOND = catalog({ id: SECOND_ID, name: "archive" });

type ListQ = Partial<ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>>;

function mockCatalogs(rows: RegistryCatalogDto[], over: ListQ = {}) {
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    ...over,
  } as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
}

function mockCatalogsError(status: number, message: string) {
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: true,
    error: new ApiError(status, message),
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
}

function Where() {
  return <div>DETAIL {useLocation().pathname + useLocation().search}</div>;
}

function renderCreate(type: "trino" | "starrocks") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderWith(qc, type);
}

function renderWith(qc: QueryClient, type: "trino" | "starrocks") {
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/engines/new?type=${type}`]}>
        <Routes>
          <Route path="/control-plane/engines/new" element={<EngineCreate />} />
          <Route path="/control-plane/engines/:id" element={<Where />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

async function attach(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole("checkbox", { name: `Attach ${name}` }));
}
async function makePrimary(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole("radio", { name: `Make ${name} primary` }));
}
const created = (over: Partial<CreateEngineResponse> = {}) =>
  ({
    id: "e2",
    name: "x",
    type: "starrocks",
    namespace: "engines",
    warehouseId: WAREHOUSE.id,
    status: "provisioning",
    spec: {},
    catalogAttachErrors: [],
    ...over,
  }) as unknown as CreateEngineResponse;

describe("EngineCreate", () => {
  beforeEach(() => {
    vi.mocked(api.listWarehouses).mockResolvedValue([WAREHOUSE]);
    vi.mocked(api.listSecrets).mockResolvedValue([]);
    vi.mocked(api.createEngine).mockClear();
    vi.mocked(api.createEngine).mockResolvedValue(created());
    mockCatalogs([EGRET]);
  });

  test("Trino: submits kind trino with a trinoConfig (coordinator/worker)", async () => {
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await makePrimary(user, "demo-catalog");
    // StarRocks-only fields must not be present on the Trino path.
    expect(screen.queryByLabelText("FE image")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.kind).toBeUndefined();
    expect(body.trinoConfig?.coordinator).toBeDefined();
    expect(body.trinoConfig?.worker).toBeDefined();
  });

  test("StarRocks: submits kind + starrocks body with defaults", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr1");
    await makePrimary(user, "demo-catalog");
    expect(screen.getByLabelText("FE image")).toBeInTheDocument();
    expect(screen.getByLabelText("CN image")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.kind).toBe("starrocks");
    expect(body.starrocks).toEqual({
      feImage: "starrocks/fe-ubuntu:4.1-latest",
      cnImage: "starrocks/cn-ubuntu:4.1-latest",
      feReplicas: 1,
      cnReplicas: 1,
    });
    // Required Trino-only body fields still sent with harmless defaults.
    expect(body.coordinatorReplicas).toBe(1);
    expect(body.memory).toBe("4Gi");
  });

  test("StarRocks: a delegation mode adds delegationMode + tlsEnabled", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr1");
    await makePrimary(user, "demo-catalog");

    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(await screen.findByRole("option", { name: "JWT (per-user)" }));
    expect(screen.getByLabelText("Enable TLS")).toBeChecked();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "jwt", tlsEnabled: true }),
    );
  });

  test("StarRocks: explicit None sends delegationMode 'none' literally", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr2");
    await makePrimary(user, "demo-catalog");

    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(
      await screen.findByRole("option", { name: "None (open engine)" }),
    );
    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "none" }),
    );
  });

  // ── Catalogs ──────────────────────────────────────────────────────────────

  test("only a catalog with a warehouse can be primary, and the row says why", async () => {
    mockCatalogs([EGRET, JDBC]);
    renderCreate("trino");

    expect(screen.getByRole("radio", { name: "Make demo-catalog primary" })).toBeEnabled();
    const jdbcRadio = screen.getByRole("radio", { name: "Make sales-pg primary" });
    expect(jdbcRadio).toBeDisabled();
    // …and the reason is in the ROW, not only in the disabled attribute. It
    // names Ranger and storage because that is what the server's own refusal
    // names, and greying a control without a reason is the thing this replaces.
    const row = jdbcRadio.closest("div.flex.items-start") as HTMLElement;
    expect(row).toHaveTextContent("Ranger role");
    expect(row).toHaveTextContent("storage");
    expect(row).toHaveTextContent("not primary");
    // Attaching it is still offered — only leading is refused.
    expect(screen.getByRole("checkbox", { name: "Attach sales-pg" })).toBeEnabled();
  });

  test("submits catalogs with exactly one primary and NO warehouseId", async () => {
    mockCatalogs([EGRET, SECOND]);
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await attach(user, "archive");
    await makePrimary(user, "demo-catalog");

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.catalogs).toEqual([
      { catalogId: CAT_ID, primary: true },
      { catalogId: SECOND_ID, primary: false },
    ]);
    // A stale `warehouseId` ALONGSIDE `catalogs` is precisely the body the
    // server has to disambiguate — it refuses it rather than picking one. The
    // absence is the assertion.
    expect(body).not.toHaveProperty("warehouseId");
  });

  test("marking a row primary also attaches it", async () => {
    mockCatalogs([EGRET, SECOND]);
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");

    const box = screen.getByRole("checkbox", { name: "Attach demo-catalog" });
    expect(box).not.toBeChecked();
    await makePrimary(user, "demo-catalog");
    expect(box).toBeChecked();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    expect(vi.mocked(api.createEngine).mock.calls[0][0].catalogs).toEqual([
      { catalogId: CAT_ID, primary: true },
    ]);
  });

  test("un-attaching the primary clears it, and blocks submit", async () => {
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await makePrimary(user, "demo-catalog");
    expect(screen.getByRole("button", { name: "Create engine" })).toBeEnabled();

    await user.click(screen.getByRole("checkbox", { name: "Attach demo-catalog" }));
    expect(screen.getByRole("radio", { name: "Make demo-catalog primary" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Create engine" })).toBeDisabled();
  });

  test("no primary blocks submit client-side", async () => {
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await attach(user, "demo-catalog");

    expect(screen.getByTestId("ec-needs-primary")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create engine" }));
    expect(api.createEngine).not.toHaveBeenCalled();
  });

  test("the server's refusal is surfaced verbatim when it happens anyway", async () => {
    vi.mocked(api.createEngine).mockRejectedValue(
      new ApiError(400, "one attached catalog must be primary"),
    );
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await makePrimary(user, "demo-catalog");
    await user.click(screen.getByRole("button", { name: "Create engine" }));

    expect(await screen.findByTestId("ec-error")).toHaveTextContent(
      "one attached catalog must be primary",
    );
    expect(screen.queryByText(/DETAIL/)).not.toBeInTheDocument();
  });

  test("an id the server cannot resolve says nothing was provisioned, and names it", async () => {
    vi.mocked(api.createEngine).mockRejectedValue(
      // The server's real words. It carries the UUID and NOT the name — so the
      // name below can only have come from the renderer resolving the id.
      new ApiError(400, `no such catalog ${CAT_ID}`),
    );
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await makePrimary(user, "demo-catalog");
    await user.click(screen.getByRole("button", { name: "Create engine" }));

    // Scoped to the alert: "demo-catalog" is also the row's own label, so an
    // unscoped query would pass on a renderer that resolved nothing.
    const alert = await screen.findByTestId("ec-error");
    expect(alert).toHaveTextContent("demo-catalog");
    expect(alert).toHaveTextContent("Nothing was provisioned");
    expect(screen.queryByText(/DETAIL/)).not.toBeInTheDocument();
  });

  test("a create with no attach errors goes straight to the engine", async () => {
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await makePrimary(user, "demo-catalog");
    await user.click(screen.getByRole("button", { name: "Create engine" }));

    expect(await screen.findByText("DETAIL /control-plane/engines/e2")).toBeInTheDocument();
    expect(screen.queryByTestId("ec-attach-errors")).not.toBeInTheDocument();
  });

  test("catalogAttachErrors are rendered instead of being navigated past", async () => {
    mockCatalogs([EGRET, SECOND]);
    vi.mocked(api.createEngine).mockResolvedValue(
      created({
        id: "e9",
        name: "analytics",
        catalogAttachErrors: [
          // The message names no catalog, so the name in the assertion can only
          // come from the renderer's own id → name resolution.
          { catalogId: SECOND_ID, message: "the engine refused the apply" },
        ],
      }),
    );
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await attach(user, "archive");
    await makePrimary(user, "demo-catalog");
    await user.click(screen.getByRole("button", { name: "Create engine" }));

    const panel = await screen.findByTestId("ec-attach-errors");
    expect(panel).toHaveTextContent("archive");
    expect(panel).toHaveTextContent("the engine refused the apply");
    // We did NOT navigate — the operator would never have seen the field.
    expect(screen.queryByText(/^DETAIL/)).not.toBeInTheDocument();
    // …and creating again is not offered, because the engine already exists.
    expect(screen.getByRole("button", { name: "Create engine" })).toBeDisabled();

    await user.click(within(panel).getByRole("button", { name: /Catalogs to retry/ }));
    expect(
      await screen.findByText("DETAIL /control-plane/engines/e9?tab=catalogs"),
    ).toBeInTheDocument();
  });

  test("a 403 on the registry names the permission and offers no catalogs", async () => {
    mockCatalogsError(403, "forbidden");
    renderCreate("trino");

    expect(screen.getByTestId("ec-catalogs-blocked")).toHaveTextContent(
      "catalog_registry:read",
    );
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    // The empty-state sentence is a claim about the registry, not about a
    // failed read of it.
    expect(screen.queryByText(/There are no catalogs to attach/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create engine" })).toBeDisabled();
  });

  test("a non-403 registry failure shows the server's own sentence", async () => {
    mockCatalogsError(500, "upstream timed out");
    renderCreate("trino");

    const blocked = screen.getByTestId("ec-catalogs-blocked");
    expect(blocked).toHaveTextContent("upstream timed out");
    expect(blocked).not.toHaveTextContent("catalog_registry:read");
  });

  test("a provisioned warehouse with no registry catalog is named, not silently absent", async () => {
    mockCatalogs([JDBC]);
    renderCreate("trino");

    expect(await screen.findByTestId("ec-unregistered-warehouses")).toHaveTextContent(
      "demo-warehouse",
    );
  });

  test("a failed registry read never claims a warehouse is unregistered", async () => {
    mockCatalogsError(403, "forbidden");
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    renderWith(qc, "trino");

    // WAIT FOR THE WAREHOUSE ROWS TO ACTUALLY BE THERE. `listWarehouses` is
    // async, and nothing else on this screen renders a warehouse — so a bare
    // "the note is absent" assertion passes while the query is still in flight,
    // which is to say it passes against the defect too. (Measured: with the
    // `status === 200` guard removed this test stayed green until this wait was
    // added.) The cache key is the shared one, imported rather than restated.
    await waitFor(() => expect(qc.getQueryData(warehousesQueryKey)).toHaveLength(1));
    // `listWarehouses` answered; the registry did not. Saying "no registry
    // catalog" here would be a statement about a read that never happened.
    expect(screen.queryByTestId("ec-unregistered-warehouses")).not.toBeInTheDocument();
  });
});
