import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The list reads the GENERATED perm client directly (no hand-written
 * wrapper), matching `Catalogs.test.tsx` — the newest list page's pattern.
 * `GrantDialog` is mocked out entirely: its own behavior (full-body PATCH,
 * preview-before-save, verbatim refusals) is covered by `GrantDialog.test.tsx`.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/hooks");
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
vi.mock("../GrantDialog", async () => {
  const actual = await vi.importActual<typeof import("../GrantDialog")>("../GrantDialog");
  return {
    ...actual,
    GrantDialog: () => null,
  };
});

import * as permApi from "@/api/generated/perm/perm";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as hooks from "@/api/hooks";
import * as govApi from "@/api/generated/control-plane-governance/control-plane-governance";
import type { Me } from "@/api/hooks";
import type { GrantView, RegistryCatalogDto } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { AuthContext } from "@/auth/useAuth";
import DataPermissions from "@/routes/governance/DataPermissions";

const CATALOG: RegistryCatalogDto = {
  id: "cat-1",
  name: "wh",
  kind: "iceberg",
  config: {} as RegistryCatalogDto["config"],
  advanced: {} as RegistryCatalogDto["advanced"],
  warehouseId: "wh-1",
  engines: [],
  workspaceIds: [],
  createdAt: "2026-09-01T00:00:00Z",
  updatedAt: "2026-09-01T00:00:00Z",
};

const ADMIN: Me = { user_id: "u1", email: "a@example.com", platform_roles: ["platform_admin"], permissions: [] };

/**
 * A grant as the READ surface hands it over — `catalog_name` included, since
 * the server resolves it (SP4b-2 finding 1). The fixture carries it rather
 * than relying on the registry mock below on purpose: the payload is where the
 * name comes from now, and a test that only worked because the registry mock
 * answered would not notice the field disappearing.
 */
function grant(over: Partial<GrantView> = {}): GrantView {
  return {
    id: "g1",
    effect: "allow",
    actions: ["read"],
    scope: { catalog_id: "cat-1", schema: "sales" },
    principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    origin: { type: "explicit" },
    catalog_name: "wh",
    ...over,
  };
}

function listResult(rows: GrantView[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListGrants
  >;
}

function renderPage(me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <DataPermissions />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(govApi.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [{ id: "cat-1", name: "wh" }] },
  } as unknown as ReturnType<typeof govApi.useControlPlaneGovernanceListCatalogs>);
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: { status: 200, data: [CATALOG] },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
  vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
    data: [{ id: "ws1", slug: "eng", name: "Engineering" }],
  } as unknown as ReturnType<typeof hooks.useMyWorkspaces>);
  vi.mocked(permApi.usePermDeleteGrant).mockReturnValue({
    mutate: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermDeleteGrant>);
});

describe("DataPermissions — the sentence is the primary column (D5)", () => {
  test("an allow grant reads as a sentence", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant()]));
    renderPage();
    expect(screen.getByText("role analyst can read wh.sales.*")).toBeInTheDocument();
  });

  test("a deny grant reads as a sentence with 'cannot'", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant({ id: "g2", effect: "deny" })]));
    renderPage();
    expect(screen.getByText("role analyst cannot read wh.sales.*")).toBeInTheDocument();
  });

  test("a principal subtract is rendered in the sentence, because it changes its meaning", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([
        grant({
          id: "g3",
          actions: ["read", "write"],
          principals: {
            include: [{ kind: "role", ref: "analyst" }],
            exclude: [{ kind: "group", ref: "contractors" }],
          },
        }),
      ]),
    );
    renderPage();
    expect(
      screen.getByText("role analyst can read and write wh.sales.* except group contractors"),
    ).toBeInTheDocument();
  });

  test("a workspace-assignment grant names its source and offers no edit", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([
        grant({
          id: "g4",
          origin: { type: "workspace_assignment", workspace_id: "ws1" },
        }),
      ]),
    );
    renderPage();
    expect(screen.getByText(/Set by workspace assignment/)).toBeInTheDocument();
    expect(screen.getByText(/Engineering/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Edit /i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Delete /i })).not.toBeInTheDocument();
  });

  test("an explicit grant DOES offer Edit and Delete to a platform_admin", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant()]));
    renderPage();
    expect(screen.getByRole("button", { name: /^Edit /i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Delete /i })).toBeInTheDocument();
  });

  test("a policy:read-only user (no policy:update) sees no Edit/Delete/New", () => {
    const reader: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant()]));
    renderPage(reader);
    expect(screen.queryByRole("button", { name: /^Edit /i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New grant" })).not.toBeInTheDocument();
  });
});

describe("DataPermissions — the sentence must not need a second permission", () => {
  /**
   * **THE DISCRIMINATING CASE (SP4b-2 finding 1).** The four governance pages
   * gate on `policy:read`; `GET /control-plane/catalogs` gates on
   * `catalog_registry:read`. Every other test in this file, and the browser
   * run's whole admin pass, resolved the catalog through that second endpoint
   * and so passed whether or not the name was on the grant payload — while a
   * `policy_reader` saw
   * `role analyst cannot read 4a8fe34a-af58-4984-a1e7-12854fb9d496.sales.orders`.
   * Here the registry answers 403, exactly as it does for that persona, so the
   * name can only come from `GrantView.catalog_name`.
   */
  test("the catalog's NAME still renders when the catalog registry answers 403", () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "orders" } })]),
    );
    renderPage({
      user_id: "u2",
      email: "r@example.com",
      platform_roles: [],
      permissions: ["policy:read"],
    });

    expect(screen.getByText("role analyst can read wh.sales.orders")).toBeInTheDocument();
    expect(screen.queryByText(/cat-1/)).not.toBeInTheDocument();
  });

  /**
   * **THE STRUCTURAL VERSION of the same guarantee (review M-6).** The 403 test
   * above proves the sentence survives a forbidden registry; this proves the
   * page never asks. A reader used to eat a guaranteed 403 on every load for a
   * lookup the page no longer needs, and a request that is never made cannot
   * come back wrong.
   */
  test("does not call the catalog registry at all", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant()]));
    renderPage();
    expect(screen.getByText("role analyst can read wh.sales.*")).toBeInTheDocument();
    expect(
      registry.useControlPlaneCatalogRegistryListRegistryCatalogs,
    ).not.toHaveBeenCalled();
  });

  test("with no name on the payload the id is shown — never a guess", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([grant({ catalog_name: null })]),
    );
    renderPage();
    expect(screen.getByText("role analyst can read cat-1.sales.*")).toBeInTheDocument();
  });
});

describe("DataPermissions: Open in Data", () => {
  test("a schema grant links to that schema's Permissions tab", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(listResult([grant()]));
    renderPage();
    expect(screen.getByRole("link", { name: /Open in Data/ })).toHaveAttribute(
      "href",
      "/governance/data/wh/sales?tab=permissions",
    );
  });

  test("a column grant links to the table with ?column=", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "orders", column: "email" } })]),
    );
    renderPage();
    expect(screen.getByRole("link", { name: /Open in Data/ })).toHaveAttribute(
      "href",
      "/governance/data/wh/sales/orders?column=email&tab=permissions",
    );
  });

  test("a pattern table links to the schema and says so", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "*" } })]),
    );
    renderPage();
    const link = screen.getByRole("link", { name: /nearest level above its pattern/ });
    expect(link).toHaveAttribute("href", "/governance/data/wh/sales?tab=permissions");
  });

  test("a catalog the Data area does not list gets no link", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue(
      listResult([grant({ scope: { catalog_id: "other", schema: "sales" } })]),
    );
    renderPage();
    expect(screen.queryByRole("link", { name: /Open in Data/ })).not.toBeInTheDocument();
  });
});
