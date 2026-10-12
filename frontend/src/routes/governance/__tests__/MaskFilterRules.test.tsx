import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@/api/generated/perm/perm");
// The profiles that resolve an overlap's service name to an editor route.
let profiles: Array<{ id: number; name: string; type: string; tag_service?: string | null }> = [];
vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  listProfiles: () => Promise.resolve(profiles),
}));
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
  profiles = [
    { id: 7, name: "trino-eu", type: "trino" },
    { id: 8, name: "trino-us", type: "trino" },
  ];
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
  test("a banner names both rules of each pair and links to each rule", async () => {
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
    await within(banner).findByRole("link", { name: "trino-eu" });
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

describe("MaskFilterRules — the overlap report reads as an operator action (SP4d-4 Task 12)", () => {
  const handPair = (over: Partial<StoredOverlap> = {}) =>
    overlap({
      rules: [
        { id: null, policyId: 31, name: "hand email" },
        { id: "r-c", policyId: 32, name: "mask emails us" },
      ],
      servedToday: null,
      howToFix:
        "Delete one of the two. A hand-written member can also be switched off with PUT /api/control-plane/policies/{id}/enabled (use its policyId).",
      ...over,
    });

  beforeEach(() => {
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
  });

  test("never shows the API route; offers a link to switch the hand-written policy off", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(overlapsResult([handPair()]));
    renderPage();
    const banner = screen.getByRole("alert");
    const link = await within(banner).findByRole("link", { name: "Switch off policy hand email" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino-eu/masking/31");
    expect(banner).toHaveTextContent("with the Enabled switch in its editor");
    expect(banner).not.toHaveTextContent("PUT");
    expect(banner).not.toHaveTextContent("/api/control-plane");
    expect(banner).not.toHaveTextContent("policyId");
    // A compiled rule has no such switch: no "switch off" link for it.
    expect(within(banner).queryByRole("link", { name: /Switch off policy mask emails us/ })).toBeNull();
  });

  test("a pair that exists only in a tag service no profile names is plain text, not a dead link", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([handPair({ serviceName: "orphan_tag", serviceId: 99 })]),
    );
    renderPage();
    const banner = screen.getByRole("alert");
    // Let the profiles query settle, then check nothing links into the tag service.
    await within(banner).findByRole("link", { name: "rule mask emails us" });
    expect(within(banner).queryByRole("link", { name: "policy hand email" })).toBeNull();
    expect(within(banner).queryByRole("link", { name: "orphan_tag" })).toBeNull();
    expect(within(banner).getByText("policy hand email")).toBeInTheDocument();
    expect(within(banner).queryByRole("link", { name: /Switch off policy/ })).toBeNull();
    for (const a of within(banner).queryAllByRole("link")) {
      expect(a.getAttribute("href")).not.toContain("orphan_tag");
    }
  });

  test("a tag service a profile names opens its policies through that profile", async () => {
    profiles = [
      { id: 7, name: "trino-eu", type: "trino", tag_service: "eu_tag" },
      { id: 70, name: "eu_tag", type: "tag" },
    ];
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([handPair({ serviceName: "eu_tag", serviceId: 70 })]),
    );
    renderPage();
    const link = await screen.findByRole("link", { name: "policy hand email" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino-eu/masking/31");
  });

  test("a tag service shared by two profiles is ONE pair, naming both profiles", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        handPair({ serviceName: "trino-eu", serviceId: 7 }),
        handPair({ serviceName: "trino-us", serviceId: 8 }),
      ]),
    );
    renderPage();
    const banner = screen.getByRole("alert");
    expect(within(banner).getByText("1 pair of rules overlaps")).toBeInTheDocument();
    expect(await within(banner).findByRole("link", { name: "trino-eu" })).toBeInTheDocument();
    expect(within(banner).getByRole("link", { name: "trino-us" })).toBeInTheDocument();
    // A different pair is still counted separately.
    expect(within(banner).getAllByText("policy hand email")).toHaveLength(1);
  });

  test("two different pairs are two pairs", async () => {
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue(
      overlapsResult([
        handPair(),
        handPair({
          serviceName: "trino-us",
          rules: [
            { id: null, policyId: 41, name: "other hand" },
            { id: "r-d", policyId: 42, name: "other rule" },
          ],
        }),
      ]),
    );
    renderPage();
    expect(screen.getByText("2 pairs of rules overlap")).toBeInTheDocument();
    await screen.findByRole("link", { name: "trino-eu" });
  });

  test("a user who may not read policies sees ONE alert, not a second from the overlap report", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof permApi.usePermListRules>);
    vi.mocked(permApi.usePermListOverlaps).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof permApi.usePermListOverlaps>);
    renderPage();
    const alerts = screen.getAllByRole("alert");
    expect(alerts).toHaveLength(1);
    expect(alerts[0]).toHaveTextContent("You need the policy:read permission");
  });

  test("a ?rule= link followed by an account that cannot edit says why the list is shown", () => {
    const reader: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
    vi.mocked(permApi.usePermListRules).mockReturnValue(
      listResult([maskRule({ id: "r-a", name: "mask emails" })]),
    );
    renderPage(reader, "/governance/mask-filter-rules?rule=r-a");
    expect(screen.queryByText(/^editing /)).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "editing needs the policy:update permission, which this account does not have",
    );
  });

  test("without a ?rule= link there is no such notice", () => {
    const reader: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
    vi.mocked(permApi.usePermListRules).mockReturnValue(listResult([maskRule()]));
    renderPage(reader);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
