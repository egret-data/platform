import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, useMutation } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The catalog detail page: Configuration (read, edit, repair) and Workspaces.
 *
 * The generated registry module is mocked **partially** — only the five hooks
 * this page calls — so the query-key BUILDERS stay real. `catalogKey` is
 * `CatalogDetail`'s own export and delegates to one of them; a literal in its
 * place would leave a saved catalog stale on screen with every other test in
 * this file green, which is exactly the defect Task 4's review found on the
 * list page.
 */
type RegistryModule = typeof import("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");

vi.mock(
  "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry",
  async (importOriginal) => ({
    ...(await importOriginal<RegistryModule>()),
    useControlPlaneCatalogRegistryGetRegistryCatalog: vi.fn(),
    useControlPlaneCatalogRegistryUpdateRegistryCatalog: vi.fn(),
    useControlPlaneCatalogRegistryCatalogImpact: vi.fn(),
    useControlPlaneCatalogRegistryListCatalogWorkspaces: vi.fn(),
    useControlPlaneCatalogRegistrySetCatalogWorkspaces: vi.fn(),
  }),
);
vi.mock("@/api/hooks");
vi.mock("@/api/controlPlane");
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type {
  ApplyOutcomeDto,
  CatalogAttachmentDto,
  CatalogImpactEngineDto,
  CatalogWorkspaceDto,
  RegistryCatalogDto,
} from "@/api/generated/model";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
import { ApiError } from "@/api/client";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { TooltipProvider } from "@/components/ui/tooltip";
import { toast } from "sonner";
import CatalogDetail, { catalogKey } from "@/routes/control-plane/catalogs/CatalogDetail";

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
    appliedAt: "2026-09-01T00:00:00Z",
    isPrimary: false,
    createdAt: "2026-09-01T00:00:00Z",
    ...over,
  };
}

/** A complete jdbc catalog — the shape `submitProblem` is happy with. */
function jdbcCatalog(over: Partial<RegistryCatalogDto> = {}): RegistryCatalogDto {
  return {
    id: "c-1",
    name: "sales",
    kind: "jdbc",
    config: {
      dialect: "postgresql",
      host: "db.internal",
      port: 5432,
      user: "egret",
      password_secret_id: "sec-1",
      database: "sales",
      ssl: { mode: "require" },
    },
    advanced: {},
    description: "The sales database",
    warehouseId: null,
    engines: [attachment()],
    workspaceIds: [],
    createdAt: "2026-08-01T10:00:00Z",
    updatedAt: "2026-09-01T10:00:00Z",
    ...over,
  };
}

function warehouseCatalog(): RegistryCatalogDto {
  return {
    id: "c-wh",
    name: "analytics",
    kind: "iceberg",
    config: { metastore: { type: "egret", warehouse_id: "wh-1" } },
    advanced: {},
    description: null,
    warehouseId: "wh-1",
    engines: [],
    workspaceIds: [],
    createdAt: "2026-08-01T10:00:00Z",
    updatedAt: "2026-09-01T10:00:00Z",
  };
}

let updateFn: ReturnType<typeof vi.fn>;
let setWorkspacesFn: ReturnType<typeof vi.fn>;

/** What the page PUT to `/catalogs/{id}`. */
function putBody() {
  expect(updateFn).toHaveBeenCalledTimes(1);
  return updateFn.mock.calls[0][0] as { id: string; data: Record<string, unknown> };
}

function mockCatalog(catalog: RegistryCatalogDto | null, error?: ApiError) {
  vi.mocked(registry.useControlPlaneCatalogRegistryGetRegistryCatalog).mockReturnValue({
    data: catalog ? { status: 200, data: catalog } : undefined,
    isLoading: false,
    isError: !!error,
    error,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryGetRegistryCatalog>);
}

function mockImpact(engines: CatalogImpactEngineDto[]) {
  vi.mocked(registry.useControlPlaneCatalogRegistryCatalogImpact).mockReturnValue({
    data: { status: 200, data: { engines } },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryCatalogImpact>);
}

function mockAssigned(rows: CatalogWorkspaceDto[]) {
  vi.mocked(registry.useControlPlaneCatalogRegistryListCatalogWorkspaces).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<
    typeof registry.useControlPlaneCatalogRegistryListCatalogWorkspaces
  >);
}

function renderDetail(path = "/control-plane/catalogs/c-1", me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[path]}>
            <Routes>
              <Route path="/control-plane/catalogs/:id" element={<CatalogDetail />} />
              <Route path="/control-plane/catalogs" element={<div>catalogs list</div>} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const setup = () => userEvent.setup({ pointerEventsCheck: 0 });
const saveButton = () => screen.queryByRole("button", { name: /^Save configuration$/ });

beforeEach(() => {
  vi.clearAllMocks();

  updateFn = vi.fn().mockResolvedValue({
    status: 200,
    data: { catalog: jdbcCatalog(), outcomes: [] as ApplyOutcomeDto[] },
  });
  setWorkspacesFn = vi.fn().mockResolvedValue({ status: 200, data: [] });

  // The mutation hooks run a REAL `useMutation` over a stub `mutationFn`, so
  // the `onSuccess`/`onError` the page attaches actually fire. A hook mocked
  // to a bare object would silently swallow both, and the outcome toasts —
  // the whole point of test 5 — could not be observed at all.
  vi.mocked(registry.useControlPlaneCatalogRegistryUpdateRegistryCatalog).mockImplementation(
    ((options?: { mutation?: Record<string, unknown> }) =>
      useMutation({ mutationFn: updateFn as never, ...options?.mutation })) as unknown as typeof registry.useControlPlaneCatalogRegistryUpdateRegistryCatalog,
  );
  vi.mocked(registry.useControlPlaneCatalogRegistrySetCatalogWorkspaces).mockImplementation(
    ((options?: { mutation?: Record<string, unknown> }) =>
      useMutation({ mutationFn: setWorkspacesFn as never, ...options?.mutation })) as unknown as typeof registry.useControlPlaneCatalogRegistrySetCatalogWorkspaces,
  );

  mockCatalog(jdbcCatalog());
  mockImpact([]);
  mockAssigned([]);

  vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
    data: [
      { id: "ws1", slug: "eng", name: "Engineering", features: [] },
      { id: "ws2", slug: "fin", name: "Finance", features: [] },
    ],
  } as unknown as ReturnType<typeof hooks.useMyWorkspaces>);
  vi.mocked(api.listStorage).mockResolvedValue([
    { id: "st-1", name: "lake" },
  ] as unknown as Awaited<ReturnType<typeof api.listStorage>>);
  vi.mocked(api.listSecrets).mockResolvedValue([
    { id: "sec-1", name: "db-pass" },
  ] as unknown as Awaited<ReturnType<typeof api.listSecrets>>);
});

describe("CatalogDetail shell", () => {
  test("renders the three tabs and the catalog's own facts", async () => {
    renderDetail();
    expect(await screen.findByRole("tab", { name: /Configuration/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Engines/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Workspaces/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "sales" })).toBeInTheDocument();
  });

  test("?tab=workspaces selects Workspaces on the first paint", async () => {
    renderDetail("/control-plane/catalogs/c-1?tab=workspaces");
    const tab = await screen.findByRole("tab", { name: /Workspaces/ });
    expect(tab).toHaveAttribute("aria-selected", "true");
    // The Configuration panel is the default; proving it is NOT the one showing
    // is what separates "the URL was read" from "the default happens to match".
    expect(screen.getByRole("tab", { name: /Configuration/ })).toHaveAttribute(
      "aria-selected",
      "false",
    );
    expect(await screen.findByRole("checkbox", { name: /Engineering/ })).toBeInTheDocument();
  });

  test("a load failure puts the server's own sentence on the page", async () => {
    mockCatalog(null, new ApiError(404, "no such catalog"));
    renderDetail();
    expect(await screen.findByText("no such catalog")).toBeInTheDocument();
  });
});

describe("Configuration tab", () => {
  test("read mode shows the stored JDBC host and offers no Save until Edit", async () => {
    const user = setup();
    renderDetail();
    expect(await screen.findByLabelText("Host")).toHaveValue("db.internal");
    // NOT `toBeDisabled` on the input: read mode wraps the form in a
    // `fieldset[disabled]`, which `toBeDisabled` walks up to — so that
    // assertion would pass whether or not the page were really read-only.
    // The absence of the Save button is the falsifiable fact.
    expect(saveButton()).toBeNull();
    await user.click(screen.getByRole("button", { name: /^Edit configuration$/ }));
    expect(saveButton()).not.toBeNull();
  });

  test("an egret warehouse catalog cannot be edited, and says why", async () => {
    mockCatalog(warehouseCatalog());
    renderDetail("/control-plane/catalogs/c-wh");
    expect(await screen.findByRole("heading", { level: 1, name: "analytics" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Edit configuration$/ })).toBeNull();
    expect(
      screen.getByText(/egret manages this catalog's configuration with its warehouse/),
    ).toBeInTheDocument();
  });

  test("editing with two engines names what saving does to each of them", async () => {
    const user = setup();
    mockCatalog(
      jdbcCatalog({
        engines: [
          attachment(),
          attachment({ engineId: "e2", engineName: "sr-eu", engineType: "starrocks" }),
        ],
      }),
    );
    mockImpact([
      { engineId: "e1", engineName: "trino-prod", engineType: "trino", action: "restart" },
      { engineId: "e2", engineName: "sr-eu", engineType: "starrocks", action: "in_place" },
    ]);
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    const blast = screen.getByTestId("catalog-impact");
    expect(within(blast).getByText(/trino-prod will restart/)).toBeInTheDocument();
    expect(within(blast).getByText(/sr-eu updates in place/)).toBeInTheDocument();
  });

  test("a partly applied save is a success: one error toast per failed engine, no error banner", async () => {
    const user = setup();
    updateFn.mockResolvedValue({
      status: 200,
      data: {
        catalog: jdbcCatalog(),
        outcomes: [
          { engineId: "e1", engineName: "trino-prod", status: "applied", error: null },
          {
            engineId: "e2",
            engineName: "sr-eu",
            status: "failed",
            error: "connection refused",
          },
        ] as ApplyOutcomeDto[],
      },
    });
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "db2.internal" } });
    await user.click(saveButton()!);

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(vi.mocked(toast.error).mock.calls[0][0]).toMatch(/sr-eu/);
    expect(vi.mocked(toast.error).mock.calls[0][0]).toMatch(/connection refused/);
    // The other engine already holds the new configuration and nothing
    // reverted, so this is not a failure: no banner, and no success toast
    // claiming everything landed.
    expect(screen.queryByTestId("catalog-config-error")).toBeNull();
    expect(toast.success).not.toHaveBeenCalled();
    expect(putBody().data.config).toMatchObject({ host: "db2.internal" });
  });

  test("a save the form knows is incomplete sends nothing and says what is missing", async () => {
    const user = setup();
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "   " } });
    // Behaviour, not `toBeDisabled`: press it and show nothing was sent.
    await user.click(saveButton()!);
    expect(updateFn).not.toHaveBeenCalled();
    expect(await screen.findByText("host is required")).toBeInTheDocument();
  });

  test("a stored configuration today's rules refuse is named, shown raw and repairable", async () => {
    const user = setup();
    // An iceberg row saved before `storage_connection_id` became mandatory
    // (`config.rs:476-478`). `GET /catalogs/{id}` hands the column back
    // unparsed, which is the whole repair path.
    mockCatalog(
      jdbcCatalog({
        id: "c-old",
        name: "legacy",
        kind: "iceberg",
        config: { metastore: { type: "hive", uri: "thrift://hms:9083" } },
        engines: [],
      }),
    );
    renderDetail("/control-plane/catalogs/c-old");
    const banner = await screen.findByTestId("catalog-config-stale");
    expect(within(banner).getByText(/storage_connection_id is required/)).toBeInTheDocument();
    // The stored bytes are on screen without being asked for: the typed form
    // cannot show a key it has no field for.
    expect(screen.getByTestId("catalog-config-json")).toHaveTextContent("thrift://hms:9083");
    // And it is repairable rather than merely legible.
    expect(
      await screen.findByRole("button", { name: /^Edit configuration$/ }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Edit configuration$/ }));
    expect(saveButton()).not.toBeNull();
  });

  /**
   * The JSON editor is the repair path. Switching back to the form used to
   * drop everything typed in it and then post the PRE-JSON draft under a
   * "Configuration saved" toast — a save that repaired nothing while claiming
   * success (review F2).
   */
  test("switching back from JSON carries the edit into the form, and into the save", async () => {
    const user = setup();
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    await user.click(screen.getByRole("button", { name: /^Edit as JSON$/ }));
    fireEvent.change(screen.getByLabelText(/Configuration JSON/), {
      target: {
        value: JSON.stringify({
          dialect: "postgresql",
          host: "repaired.internal",
          port: 5432,
          user: "egret",
          password_secret_id: "sec-1",
          database: "sales",
          ssl: { mode: "require" },
        }),
      },
    });
    await user.click(screen.getByRole("button", { name: /^Edit as a form$/ }));
    // The form took the typed value over rather than reverting to the stored one.
    expect(screen.getByLabelText("Host")).toHaveValue("repaired.internal");
    await user.click(saveButton()!);
    await waitFor(() => expect(updateFn).toHaveBeenCalledTimes(1));
    expect(putBody().data.config).toMatchObject({ host: "repaired.internal" });
  });

  test("unparseable JSON refuses the switch to the form instead of discarding it", async () => {
    const user = setup();
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    await user.click(screen.getByRole("button", { name: /^Edit as JSON$/ }));
    fireEvent.change(screen.getByLabelText(/Configuration JSON/), {
      target: { value: '{"host": "half-typed' },
    });
    await user.click(screen.getByRole("button", { name: /^Edit as a form$/ }));
    // Still in the JSON editor, with the text intact, and told why.
    expect(screen.getByLabelText(/Configuration JSON/)).toHaveValue('{"host": "half-typed');
    expect(screen.getByTestId("catalog-config-error")).toHaveTextContent(/not valid JSON/);
  });

  /**
   * The banner's silence is reachable TODAY — a stored jdbc row with an
   * unknown dialect and a non-UUID secret id shows nothing, because
   * `submitProblem` is a subset of `validate_config`. A quiet page must
   * therefore say what quiet means (review F3).
   */
  test("read mode bounds what the staleness check actually checked", async () => {
    renderDetail();
    expect(await screen.findByTestId("catalog-config-check-bound")).toHaveTextContent(
      /checks only the rules it knows/i,
    );
  });

  /**
   * `submitProblem` guarded every nested read but not the top-level one, so a
   * stored `config` of JSON `null` threw inside the render and took the back
   * link, the header and the tabs with it — on the one page whose job is to
   * show a row nothing can parse (review F4).
   */
  test("a stored config of JSON null renders the page instead of blanking it", async () => {
    mockCatalog(jdbcCatalog({ config: null as unknown as RegistryCatalogDto["config"] }));
    renderDetail();
    expect(await screen.findByRole("heading", { level: 1, name: "sales" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Configuration/ })).toBeInTheDocument();
    expect(screen.getByTestId("catalog-config-json")).toBeInTheDocument();
  });

  test("per-engine advanced properties are in the read pass, not only inside the JSON disclosure", async () => {
    mockCatalog(
      jdbcCatalog({
        advanced: { trino: { "join-distribution-type": "BROADCAST" } },
      }),
    );
    renderDetail();
    const adv = await screen.findByTestId("catalog-advanced-read");
    expect(adv).toHaveTextContent("join-distribution-type");
    expect(adv).toHaveTextContent("BROADCAST");
    expect(adv).toHaveTextContent("trino");
  });

  test("the impact headline counts the engines the impact query named", async () => {
    const user = setup();
    mockCatalog(
      jdbcCatalog({
        engines: [
          attachment(),
          attachment({ engineId: "e2", engineName: "sr-eu", engineType: "starrocks" }),
        ],
      }),
    );
    // The query answers for one engine only — attachments changed under the
    // page. The headline must not say "2 engines" above a single bullet.
    mockImpact([
      { engineId: "e1", engineName: "trino-prod", engineType: "trino", action: "restart" },
    ]);
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    const blast = screen.getByTestId("catalog-impact");
    expect(blast).toHaveTextContent("Saving re-applies this catalog to 1 engine");
    // The headline must not count the attachments while the bullets count the
    // impact answer — "2 engines" above one bullet is the defect.
    expect(blast).not.toHaveTextContent("to 2 engines");
  });

  test("the JSON editor posts exactly what was typed, for a shape no form has", async () => {
    const user = setup();
    mockCatalog(
      jdbcCatalog({ kind: "some_future_kind", config: { anything: 1 }, engines: [] }),
    );
    renderDetail();
    await user.click(await screen.findByRole("button", { name: /^Edit configuration$/ }));
    fireEvent.change(screen.getByLabelText(/Configuration JSON/), {
      target: { value: '{"anything": 2}' },
    });
    await user.click(saveButton()!);
    await waitFor(() => expect(updateFn).toHaveBeenCalledTimes(1));
    expect(putBody().data.config).toEqual({ anything: 2 });
  });
});

describe("Workspaces tab", () => {
  test("saving sends the WHOLE assignment set, not the additions", async () => {
    const user = setup();
    mockCatalog(jdbcCatalog({ workspaceIds: ["ws1", "ws2"] }));
    mockAssigned([
      { workspaceId: "ws1", name: "Engineering", slug: "eng", assignedAt: "2026-09-01T00:00:00Z" },
      { workspaceId: "ws2", name: "Finance", slug: "fin", assignedAt: "2026-09-02T00:00:00Z" },
    ]);
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: [
        { id: "ws1", slug: "eng", name: "Engineering", features: [] },
        { id: "ws2", slug: "fin", name: "Finance", features: [] },
        { id: "ws3", slug: "ops", name: "Operations", features: [] },
      ],
    } as unknown as ReturnType<typeof hooks.useMyWorkspaces>);

    renderDetail("/control-plane/catalogs/c-1?tab=workspaces");
    await user.click(await screen.findByRole("checkbox", { name: /Operations/ }));
    await user.click(screen.getByRole("button", { name: /^Save assignments$/ }));

    await waitFor(() => expect(setWorkspacesFn).toHaveBeenCalledTimes(1));
    const body = setWorkspacesFn.mock.calls[0][0] as {
      id: string;
      data: { workspaceIds: string[] };
    };
    expect(body.id).toBe("c-1");
    // A PUT is a REPLACE. Sending only `["ws3"]` would unassign Engineering
    // and Finance without a word on screen.
    expect(body.data.workspaceIds).toHaveLength(3);
    expect([...body.data.workspaceIds].sort()).toEqual(["ws1", "ws2", "ws3"]);
  });

  test("an assigned workspace the operator is not a member of keeps its row and its assignment", async () => {
    const user = setup();
    // `useMyWorkspaces` is the CALLER's workspaces; the assignment list is not
    // so restricted. A row set built from the caller's list alone would drop
    // `ws9` from a full-set PUT — a silent unassign.
    mockCatalog(jdbcCatalog({ workspaceIds: ["ws9"] }));
    mockAssigned([
      { workspaceId: "ws9", name: "Risk", slug: "risk", assignedAt: "2026-09-03T00:00:00Z" },
    ]);
    renderDetail("/control-plane/catalogs/c-1?tab=workspaces");
    expect(await screen.findByRole("checkbox", { name: /Risk/ })).toBeChecked();
    await user.click(screen.getByRole("checkbox", { name: /Engineering/ }));
    await user.click(screen.getByRole("button", { name: /^Save assignments$/ }));
    await waitFor(() => expect(setWorkspacesFn).toHaveBeenCalledTimes(1));
    const ids = (
      setWorkspacesFn.mock.calls[0][0] as { data: { workspaceIds: string[] } }
    ).data.workspaceIds;
    expect([...ids].sort()).toEqual(["ws1", "ws9"]);
  });

  test("a read-only operator cannot save, and the tab does not overstate what assignment does", async () => {
    mockAssigned([]);
    renderDetail("/control-plane/catalogs/c-1?tab=workspaces", READER);
    expect(await screen.findByRole("checkbox", { name: /Engineering/ })).toBeInTheDocument();
    // No click: the box is `disabled={!canUpdate}`, so clicking it is inert and
    // would read as if it had proved something. The absence of the Save button
    // is the assertion that carries this test (review test-quality note).
    expect(screen.queryByRole("button", { name: /^Save assignments$/ })).toBeNull();
    expect(
      screen.getByText(/It does not restrict what an engine will answer/),
    ).toBeInTheDocument();
  });

  /**
   * `list_engine_catalogs` (`egret-query/src/registry/repo.rs:509-538`) ORs the
   * assignment test with `warehouse_id IS NOT NULL AND NOT EXISTS (… any
   * assignment at all …) AND the caller owns the warehouse`. So the fallback
   * exists AND the first assignment — to anyone — destroys it. Both halves
   * change what an operator should do and neither was on screen.
   */
  test("the tab states the ownership fallback and that the first assignment replaces it", async () => {
    mockAssigned([]);
    renderDetail("/control-plane/catalogs/c-1?tab=workspaces");
    const note = await screen.findByTestId("workspace-assignment-rules");
    expect(note).toHaveTextContent(/no assignments at all/);
    expect(note).toHaveTextContent(/the first assignment replaces that/i);
    // The consequence, not only the rule: assigning it elsewhere takes it away
    // from the owner.
    expect(note).toHaveTextContent(/takes it out of the owning workspace/i);
  });

  test("a still-loading workspace list is a skeleton, not the sentence 'no workspaces'", async () => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: undefined,
      isLoading: true,
    } as unknown as ReturnType<typeof hooks.useMyWorkspaces>);
    mockAssigned([]);
    renderDetail("/control-plane/catalogs/c-1?tab=workspaces");
    expect(await screen.findByTestId("workspace-assignment-rules")).toBeInTheDocument();
    expect(screen.queryByText("There are no workspaces to assign.")).toBeNull();
  });
});

/**
 * `catalogKey` is what every mutation on this page invalidates. The generated
 * detail query's key is its URL — `["/api/control-plane/catalogs/<id>"]` — not
 * the plausible-looking `["control-plane","catalogs",id]` triple. A literal
 * there matches no cache entry, so a saved catalog stays on screen exactly as
 * it was, with every other test in this file still green.
 */
describe("catalogKey", () => {
  test("is the generated detail query's own key", () => {
    expect(catalogKey("c-1")).toEqual(
      registry.getControlPlaneCatalogRegistryGetRegistryCatalogQueryKey("c-1"),
    );
    expect(catalogKey("c-1")).toEqual(["/api/control-plane/catalogs/c-1"]);
    // And it encodes, which a hand-written triple would not.
    expect(catalogKey("a/b")[0]).toContain("a%2Fb");
  });
});
