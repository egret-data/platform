import { render, screen } from "@testing-library/react";
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
import type { Me } from "@/api/hooks";
import type { Grant, RegistryCatalogDto } from "@/api/generated/model";
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

function grant(over: Partial<Grant> = {}): Grant {
  return {
    id: "g1",
    effect: "allow",
    actions: ["read"],
    scope: { catalog_id: "cat-1", schema: "sales" },
    principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    origin: { type: "explicit" },
    ...over,
  };
}

function listResult(rows: Grant[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListGrants
  >;
}

function renderPage(me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <DataPermissions />
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
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
