import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/tags");
vi.mock("../RuleDialog", async () => {
  const actual = await vi.importActual<typeof import("../RuleDialog")>("../RuleDialog");
  return { ...actual, RuleDialog: () => null };
});

import * as permApi from "@/api/generated/perm/perm";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as tagsApi from "@/api/tags";
import type { Me } from "@/api/hooks";
import type { RegistryCatalogDto, Rule } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import MaskFilterRules from "@/routes/governance/MaskFilterRules";

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

function maskRule(over: Partial<Rule> = {}): Rule {
  return {
    id: "r1",
    name: "mask-ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key: "pii_v2" },
    exempt: { include: [], exclude: [] },
    ...over,
  };
}

function listResult(rows: Rule[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListRules
  >;
}

function renderPage(me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MaskFilterRules />
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
  vi.mocked(tagsApi.useTags).mockReturnValue({
    data: [{ id: "t1", key: "pii_v2", name: "PII Data", description: "", color: "#fff", assignment_count: 0, created_at: "", required_roles: [], required_groups: [] }],
  } as unknown as ReturnType<typeof tagsApi.useTags>);
  vi.mocked(permApi.usePermDeleteRule).mockReturnValue({
    mutate: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermDeleteRule>);
});

describe("MaskFilterRules list", () => {
  test("shows name, kind, what it selects (resolved to the tag's NAME) and exemptions", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([
        maskRule({
          exempt: { include: [{ kind: "user", ref: "alice" }], exclude: [] },
        }),
      ]),
    );
    renderPage();

    expect(screen.getByText("mask-ssn")).toBeInTheDocument();
    expect(screen.getByText("mask · MASK_HASH")).toBeInTheDocument();
    expect(screen.getByText("tag PII Data")).toBeInTheDocument();
    expect(screen.getByText("user alice")).toBeInTheDocument();
  });

  test("a row-filter rule selecting by path shows the resolved scope", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([
        maskRule({
          id: "r2",
          name: "filter-region",
          kind: { type: "row_filter", filter_expr: "region = 'us'" },
          selector: { type: "path", catalog_id: "cat-1", schema: "sales", table: "orders" },
        }),
      ]),
    );
    renderPage();

    expect(screen.getByText("row filter · region = 'us'")).toBeInTheDocument();
    expect(screen.getByText("wh.sales.orders")).toBeInTheDocument();
  });

  test("a rule with no exemptions shows a dash, not an empty cell", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    renderPage();
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  test("a policy:read-only user sees no Edit/Delete/New", () => {
    const reader: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    renderPage(reader);
    expect(screen.queryByRole("button", { name: /^Edit /i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New rule" })).not.toBeInTheDocument();
  });
});
