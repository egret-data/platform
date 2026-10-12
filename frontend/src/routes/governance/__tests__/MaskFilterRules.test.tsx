import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("../RuleDialog", async () => {
  const actual = await vi.importActual<typeof import("../RuleDialog")>("../RuleDialog");
  // A stand-in that shows which rule the page opened for editing.
  return {
    ...actual,
    RuleDialog: ({ open, existing }: { open: boolean; existing: { name: string } | null }) =>
      open ? <div>{`editing ${existing?.name ?? "nothing"}`}</div> : null,
  };
});

import * as permApi from "@/api/generated/perm/perm";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { Me } from "@/api/hooks";
import type { RegistryCatalogDto, RuleView, StoredOverlap } from "@/api/generated/model";
import { ApiError } from "@/api/client";
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

/**
 * A rule as the READ surface hands it over. `tag_name`/`catalog_name` are
 * resolved server-side (SP4b-2 finding 1) — `GET /tags` needs `tag:read` and
 * the catalog registry needs `catalog_registry:read`, neither of which
 * `policy:read` implies, so a client-side lookup showed a reader the stored
 * key (`tag pii_v2`) instead of the display name.
 */
function maskRule(over: Partial<RuleView> = {}): RuleView {
  return {
    id: "r1",
    name: "mask-ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key: "pii_v2" },
    exempt: { include: [], exclude: [] },
    tag_name: "PII Data",
    ...over,
  };
}

function listResult(rows: RuleView[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListRules
  >;
}

function overlap(over: Partial<StoredOverlap> = {}): StoredOverlap {
  return {
    serviceId: 7,
    serviceName: "trino-eu",
    kind: "mask",
    coordinate: "catalog=[wh] column=[email] schema=[sales] table=[orders]",
    rules: [
      { id: "r-a", policyId: 11, name: "mask emails" },
      { id: "r-b", policyId: 12, name: "null orders email" },
    ],
    servedToday: { id: "r-a", policyId: 11, name: "mask emails" },
    sentence: 'two mask policies would be served at policyPriority 0 ...',
    howToFix: "Delete one of the two, or narrow one so they no longer select a common resource.",
    ...over,
  };
}

function overlapsResult(rows: StoredOverlap[]) {
  return { data: { status: 200, data: rows }, isLoading: false, isError: false, error: null } as unknown as ReturnType<
    typeof permApi.usePermListOverlaps
  >;
}

function renderPage(me: Me | null = ADMIN, url = "/governance/mask-filter-rules") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter initialEntries={[url]}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <QueryClientProvider client={qc}>
          <MaskFilterRules />
        </QueryClientProvider>
      </AuthContext.Provider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: { status: 200, data: [CATALOG] },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
  vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([]));
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
          tag_name: null,
          catalog_name: "wh",
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

describe("MaskFilterRules — the Selects column must not need a second permission", () => {
  /**
   * **THE DISCRIMINATING CASE (SP4b-2 finding 1, the rule half).** Both name
   * sources answer 403 here, exactly as they do for a `policy:read`-only
   * holder — the browser run measured `tag pii` where a steward should read
   * `tag PII`. The names can only come from the rule payload.
   */
  test("the tag's DISPLAY name and the catalog's name render with both lookups forbidden", () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([
        maskRule(),
        maskRule({
          id: "r2",
          name: "filter-region",
          kind: { type: "row_filter", filter_expr: "region = 'us'" },
          selector: { type: "path", catalog_id: "cat-1", schema: "sales", table: "orders" },
          tag_name: null,
          catalog_name: "wh",
        }),
      ]),
    );
    renderPage({
      user_id: "u2",
      email: "r@example.com",
      platform_roles: [],
      permissions: ["policy:read"],
    });

    expect(screen.getByText("tag PII Data")).toBeInTheDocument();
    expect(screen.queryByText("tag pii_v2")).not.toBeInTheDocument();
    expect(screen.getByText("wh.sales.orders")).toBeInTheDocument();
  });
});

describe("MaskFilterRules — the page asks for no lookup it does not need (review M-6)", () => {
  /** The structural half of finding 1: not "the 403 is survivable" but "no
   *  request is made". A reader used to eat a guaranteed 403 per load. */
  test("does not call the catalog registry at all", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    renderPage();
    expect(screen.getByText("tag PII Data")).toBeInTheDocument();
    expect(
      registry.useControlPlaneCatalogRegistryListRegistryCatalogs,
    ).not.toHaveBeenCalled();
  });
});

describe("MaskFilterRules — the overlap report", () => {
  test("a banner names both rules of each pair and links to each rule", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        overlap(),
        overlap({
          serviceId: 8,
          serviceName: "trino-us",
          rules: [
            { id: null, policyId: 31, name: "hand email" },
            { id: "r-c", policyId: 32, name: "mask emails us" },
          ],
          servedToday: null,
        }),
      ]),
    );
    renderPage();

    const banner = screen.getByRole("alert");
    expect(within(banner).getByText("2 pairs of rules overlap")).toBeInTheDocument();
    expect(within(banner).getByRole("link", { name: "rule mask emails" })).toHaveAttribute(
      "href",
      "/governance/mask-filter-rules?rule=r-a",
    );
    expect(within(banner).getByRole("link", { name: "rule null orders email" })).toHaveAttribute(
      "href",
      "/governance/mask-filter-rules?rule=r-b",
    );
    // A hand-written member opens its policy in that profile's editor.
    expect(within(banner).getByRole("link", { name: "policy hand email" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-us/masking/31",
    );
    expect(within(banner).getByRole("link", { name: "trino-eu" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-eu",
    );
    expect(within(banner).getByText(/today the engine serves mask emails\./)).toBeInTheDocument();
  });

  test("one pair reads in the singular", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([overlap()]));
    renderPage();
    expect(screen.getByText("1 pair of rules overlaps")).toBeInTheDocument();
  });

  test("an empty report shows no banner", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    renderPage();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("mask-ssn")).toBeInTheDocument();
  });

  test("a report that fails to load says so rather than showing nothing", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
    } as unknown as ReturnType<typeof permApi.usePermListOverlaps>);
    renderPage();
    expect(screen.getByRole("alert")).toHaveTextContent("Could not check for overlapping rules");
  });

  test("following a rule link opens that rule for editing", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([maskRule({ id: "r-a", name: "mask emails" })]),
    );
    renderPage(ADMIN, "/governance/mask-filter-rules?rule=r-a");
    expect(screen.getByText("editing mask emails")).toBeInTheDocument();
  });

  test("a reader who cannot edit is not sent into the editor", () => {
    const reader: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([maskRule({ id: "r-a", name: "mask emails" })]),
    );
    renderPage(reader, "/governance/mask-filter-rules?rule=r-a");
    expect(screen.queryByText(/^editing /)).not.toBeInTheDocument();
  });
});
