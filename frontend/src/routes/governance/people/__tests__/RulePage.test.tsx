import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * `/governance/rules/:id` (spec §2 "Single items", P8): one mask or row-filter
 * rule: its kind, what it selects (a path, with Open in Data; a tag, with its
 * classification page when the viewer holds `tag:read`), its exemptions
 * linked to their People pages, and Edit and Delete with `policy:update`. The
 * Mask & filter rules list's tests of these facts live here now. `RuleDialog`
 * is a stand-in that says which rule it was opened on (`RuleDialog.test.tsx`
 * covers the dialog).
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
vi.mock("@/api/tags");
vi.mock("@/routes/governance/RuleDialog", async () => {
  const actual = await vi.importActual<typeof import("@/routes/governance/RuleDialog")>("@/routes/governance/RuleDialog");
  return {
    ...actual,
    RuleDialog: ({ open, existing }: { open: boolean; existing: { name: string } | null }) =>
      open ? <div>{`editing ${existing?.name ?? "nothing"}`}</div> : null,
  };
});

import * as permApi from "@/api/generated/perm/perm";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as govApi from "@/api/generated/control-plane-governance/control-plane-governance";
import * as tagsApi from "@/api/tags";
import type { Me } from "@/api/hooks";
import type { RuleView } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { AuthContext } from "@/auth/useAuth";
import RulePage from "@/routes/governance/people/RulePage";

const ADMIN: Me = { user_id: "u1", email: "a@example.com", platform_roles: ["platform_admin"], permissions: [] };
const WRITER: Me = { user_id: "u3", email: "w@example.com", platform_roles: [], permissions: ["policy:read", "policy:update", "tag:read"] };
const READER: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };
const TAG_READER: Me = { user_id: "u4", email: "t@example.com", platform_roles: [], permissions: ["policy:read", "tag:read"] };

/**
 * A rule as the read surface hands it over. `tag_name` and `catalog_name` are
 * resolved server-side (SP4b-2 finding 1): `GET /tags` needs `tag:read` and
 * the registry `catalog_registry:read`, neither of which `policy:read` implies.
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

const pathRule = (over: Partial<RuleView> = {}) =>
  maskRule({
    id: "r2",
    name: "filter-region",
    kind: { type: "row_filter", filter_expr: "region = 'us'" },
    selector: { type: "path", catalog_id: "cat-1", schema: "sales", table: "orders" },
    tag_name: null,
    catalog_name: "wh",
    ...over,
  });

function serveRules(rows: RuleView[]) {
  vi.mocked(permApi.usePermListRules).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermListRules>);
}

let deleteMutate: ReturnType<typeof vi.fn>;

function renderPage(id = "r1", me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/governance/rules/${id}`]}>
          <Routes>
            <Route path="/governance/rules/:id" element={<RulePage />} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const detail = (label: string) => screen.getByText(label, { selector: "dt" }).nextElementSibling as HTMLElement;

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(govApi.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [{ id: "cat-1", name: "wh", workspaces: [{ id: "ws9", name: "Analytics", slug: "analytics" }] }] },
  } as unknown as ReturnType<typeof govApi.useControlPlaneGovernanceListCatalogs>);
  vi.mocked(tagsApi.useTags).mockReturnValue({
    data: [{ id: "tag-9", key: "pii_v2", name: "PII Data" }],
  } as unknown as ReturnType<typeof tagsApi.useTags>);
  deleteMutate = vi.fn();
  vi.mocked(permApi.usePermDeleteRule).mockReturnValue({
    mutate: deleteMutate,
  } as unknown as ReturnType<typeof permApi.usePermDeleteRule>);
});

describe("RulePage: what the rule is", () => {
  test("names the rule, its kind, the tag it selects by NAME, and its exemptions linked", () => {
    serveRules([
      maskRule({
        exempt: { include: [{ kind: "user", ref: "alice" }], exclude: [{ kind: "group", ref: "temps" }] },
      }),
      pathRule(),
    ]);
    renderPage();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("mask-ssn");
    expect(detail("Kind")).toHaveTextContent("mask · MASK_HASH");
    expect(detail("Selects")).toHaveTextContent("tag PII Data");
    expect(detail("Exempt")).toHaveTextContent("user alice except group temps");
    expect(within(detail("Exempt")).getByRole("link", { name: "alice" })).toHaveAttribute(
      "href",
      "/governance/people/users/by-name/alice",
    );
    expect(within(detail("Exempt")).getByRole("link", { name: "temps" })).toHaveAttribute(
      "href",
      "/governance/people/groups/temps",
    );
    expect(screen.queryByText("filter-region")).not.toBeInTheDocument();
  });

  test("a workspace's members in an exemption read by the workspace's name", () => {
    serveRules([maskRule({ exempt: { include: [{ kind: "workspace_members", ref: "ws9" }], exclude: [] } })]);
    renderPage();
    expect(detail("Exempt")).toHaveTextContent("members of workspace Analytics");
    expect(screen.queryByText(/ws9/)).not.toBeInTheDocument();
  });

  test("a rule that exempts nobody says so", () => {
    serveRules([maskRule()]);
    renderPage();
    expect(detail("Exempt")).toHaveTextContent("nobody");
  });

  test("a row-filter rule selecting by path shows its expression and the resolved scope", () => {
    serveRules([pathRule()]);
    renderPage("r2");
    expect(detail("Kind")).toHaveTextContent("row filter · region = 'us'");
    expect(detail("Selects")).toHaveTextContent("wh.sales.orders");
  });
});

describe("RulePage: names that need no second permission", () => {
  /** SP4b-2 finding 1, the rule half: both lookups answer 403 for a `policy:read`-only holder. */
  test("the tag's display name and the catalog's name render with both lookups forbidden", () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
    serveRules([maskRule(), pathRule()]);
    const { unmount } = renderPage("r1", READER);
    expect(detail("Selects")).toHaveTextContent("tag PII Data");
    expect(screen.queryByText(/pii_v2/)).not.toBeInTheDocument();
    unmount();
    renderPage("r2", READER);
    expect(detail("Selects")).toHaveTextContent("wh.sales.orders");
  });

  test("does not call the catalog registry at all", () => {
    serveRules([maskRule()]);
    renderPage();
    expect(detail("Selects")).toHaveTextContent("tag PII Data");
    expect(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).not.toHaveBeenCalled();
  });
});

describe("RulePage: Open in Data, or the tag", () => {
  test("a tag rule opens its classification for a tag:read holder", () => {
    serveRules([maskRule()]);
    renderPage("r1", TAG_READER);
    expect(screen.getByRole("link", { name: /Open classification/ })).toHaveAttribute(
      "href",
      "/governance/classifications/tag-9?tab=rules",
    );
  });

  test("without tag:read the tag is text, and the tag list is never asked for", () => {
    serveRules([maskRule()]);
    renderPage("r1", READER);
    expect(screen.queryByRole("link", { name: /Open classification/ })).not.toBeInTheDocument();
    expect(tagsApi.useTags).not.toHaveBeenCalled();
  });

  test("a tag rule whose key the tag list does not hold gets no link, never a broken one", () => {
    serveRules([maskRule({ selector: { type: "tag", key: "gone" }, tag_name: "PII Data" })]);
    renderPage("r1", TAG_READER);
    expect(detail("Selects")).toHaveTextContent("tag PII Data");
    expect(screen.queryByRole("link", { name: /Open classification/ })).not.toBeInTheDocument();
  });

  test("a tag rule whose tag no longer exists says so and links nowhere", () => {
    serveRules([maskRule({ selector: { type: "tag", key: "gone" }, tag_name: null })]);
    renderPage("r1", TAG_READER);
    expect(detail("Selects")).toHaveTextContent("tag gone");
    expect(screen.getByText("No tag has the key gone now.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Open classification/ })).not.toBeInTheDocument();
  });

  test("a column path rule opens the table's Masks & filters tab", () => {
    serveRules([
      maskRule({ selector: { type: "path", catalog_id: "cat-1", schema: "sales", table: "orders", column: "email" } }),
    ]);
    renderPage();
    expect(screen.getByRole("link", { name: /Open in Data/ })).toHaveAttribute(
      "href",
      "/governance/data/wh/sales/orders?column=email&tab=masks",
    );
  });
});

describe("RulePage: Edit and Delete", () => {
  test("policy:update offers both; Edit opens the rule dialog on this rule", async () => {
    serveRules([maskRule()]);
    renderPage("r1", WRITER);
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByText("editing mask-ssn")).toBeInTheDocument();
  });

  test("policy:read alone offers neither, and says what editing needs", () => {
    serveRules([maskRule()]);
    renderPage("r1", READER);
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.queryByText(/^editing /)).not.toBeInTheDocument();
    expect(screen.getByText("Editing or deleting a rule needs the policy:update permission.")).toBeInTheDocument();
  });

  test("Delete asks first, then deletes this rule", async () => {
    serveRules([maskRule()]);
    renderPage("r1", WRITER);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent('Delete "mask-ssn"?');
    await userEvent.type(within(confirm).getByRole("textbox"), "delete");
    await userEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(deleteMutate).toHaveBeenCalledWith({ id: "r1" });
  });
});

describe("RulePage: states", () => {
  test("an id no rule has says so", () => {
    serveRules([maskRule()]);
    renderPage("r-gone");
    expect(screen.getByText("No rule has this id; it may have been deleted.")).toBeInTheDocument();
  });

  test("a failed read shows its error with a retry, never the missing state", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof permApi.usePermListRules>);
    renderPage();
    expect(screen.getByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(screen.queryByText(/No rule has this id/)).not.toBeInTheDocument();
  });

  test("loading shows a skeleton, not the missing state", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    } as unknown as ReturnType<typeof permApi.usePermListRules>);
    renderPage();
    expect(screen.queryByText(/No rule has this id/)).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Rule");
  });
});
