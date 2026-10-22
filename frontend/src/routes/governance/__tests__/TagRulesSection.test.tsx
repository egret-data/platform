import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * A classification's "Rules" (DG redesign #2, P9; renamed from "Mask and
 * filter rules" by the classification UX glossary): every rule a
 * classification-selected mask or row filter reaches it through. Engines match a
 * tag rule by the tag's NAME (case-sensitively, `*`/`?` glob), so a rule
 * written on another tag of the same name is listed too, and says so.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");

import * as permApi from "@/api/generated/perm/perm";
import * as govApi from "@/api/generated/control-plane-governance/control-plane-governance";
import type { RuleView } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import TagRulesSection from "@/routes/governance/TagRulesSection";

const TAG = { key: "pii_eu", name: "PII" };

function tagRule(over: Partial<RuleView> & { key?: string }): RuleView {
  const { key = "pii_eu", ...rest } = over;
  return {
    id: "r1",
    name: "mask ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key },
    exempt: { include: [], exclude: [] },
    tag_name: "PII",
    ...rest,
  };
}

function serveRules(rows: RuleView[]) {
  vi.mocked(permApi.usePermListRules).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermListRules>);
}

function renderSection() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <TagRulesSection tag={TAG} tagId="t-eu" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const region = () => screen.getByRole("region", { name: "Rules" });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(govApi.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [] },
  } as unknown as ReturnType<typeof govApi.useControlPlaneGovernanceListCatalogs>);
});

describe("TagRulesSection", () => {
  test("lists each rule whose tag name matches, linked to its page; a rule on another tag of the same name says so", () => {
    serveRules([
      tagRule({ id: "r-own", name: "mask ssn", exempt: { include: [{ kind: "role", ref: "auditor" }], exclude: [] } }),
      tagRule({ id: "r-us", name: "null email", key: "pii_us", kind: { type: "row_filter", filter_expr: "region = 'eu'" } }),
      tagRule({ id: "r-glob", name: "hash all P", key: "p", tag_name: "P*" }),
      tagRule({ id: "r-lower", name: "lowercase", key: "pii_low", tag_name: "pii" }),
      tagRule({ id: "r-gone", name: "orphan", tag_name: null }),
      {
        id: "r-path",
        name: "path rule",
        kind: { type: "mask", mask_type: "MASK_NULL" },
        selector: { type: "path", catalog_id: "c", schema: "s" },
        exempt: { include: [], exclude: [] },
        tag_name: null,
      },
    ]);
    renderSection();
    const rows = within(region()).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["hash all P", "mask ssn", "null email"]);

    expect(within(region()).getByRole("link", { name: "mask ssn" })).toHaveAttribute(
      "href",
      "/governance/rules/r-own?from=classification:t-eu",
    );
    expect(within(region()).getByRole("link", { name: "null email" })).toHaveAttribute(
      "href",
      "/governance/rules/r-us?from=classification:t-eu",
    );
    expect(rows[1]).toHaveTextContent("This classification");
    expect(rows[1]).toHaveTextContent("Mask · Hash");
    expect(within(rows[1]).getByRole("link", { name: "auditor" })).toHaveAttribute("href", "/governance/people/roles/auditor");
    expect(rows[2]).toHaveTextContent("Classification pii_us, which has the same name");
    expect(rows[2]).toHaveTextContent("Row filter · region = 'eu'");
    // The label is words; only the expression is code.
    expect(within(rows[2]).getByText("region = 'eu'").tagName).toBe("CODE");
    expect(within(rows[1]).queryByRole("code")).not.toBeInTheDocument();
    expect(within(rows[1]).getAllByRole("cell")[1].className).not.toMatch(/font-mono/);
    expect(rows[0]).toHaveTextContent("Classification p, whose name “P*” matches this one's");
    expect(region()).not.toHaveTextContent("lowercase");
    expect(region()).not.toHaveTextContent("orphan");
    expect(region()).not.toHaveTextContent("path rule");
    for (const retired of ["Reaches this tag", "This tag", "Written on tag"]) expect(region()).not.toHaveTextContent(retired);
    expect(within(region()).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "Rule",
      "Kind",
      "Applies through",
      "Exempt",
    ]);
    expect(region()).toHaveTextContent(
      "Rules that mask or filter data carrying this classification. Engines match a rule by the classification's name, so a rule written on another classification with the same name applies here too.",
    );
  });

  test("says when no rule reaches the tag", () => {
    serveRules([tagRule({ tag_name: "Finance", key: "fin" })]);
    renderSection();
    expect(within(region()).getByText("No rule uses this classification yet.")).toBeInTheDocument();
  });

  test("a failed read shows its error with a retry, never the empty state", () => {
    vi.mocked(permApi.usePermListRules).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof permApi.usePermListRules>);
    renderSection();
    expect(within(region()).getByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(region()).queryByText("No rule uses this classification yet.")).not.toBeInTheDocument();
  });
});

describe("TagRulesSection: enforcement", () => {
  test("a rule no engine enforces says so on its row; an enforced one says nothing", async () => {
    const { AuthContext } = await import("@/auth/useAuth");
    serveRules([tagRule({ id: "r-off", name: "mask ssn" }), tagRule({ id: "r-on", name: "hash email" })]);
    vi.mocked(permApi.usePermRulesEnforcement).mockReturnValue({
      data: {
        status: 200,
        data: [
          { ruleId: "r-off", enforcedBy: 0, notEnforced: "no_engine" },
          { ruleId: "r-on", enforcedBy: 1, notEnforced: null },
        ],
      },
    } as unknown as ReturnType<typeof permApi.usePermRulesEnforcement>);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <AuthContext.Provider value={{ me: { user_id: "u", email: "", platform_roles: [], permissions: ["policy:read"] }, isLoading: false }}>
          <MemoryRouter>
            <TagRulesSection tag={TAG} tagId="t-eu" />
          </MemoryRouter>
        </AuthContext.Provider>
      </QueryClientProvider>,
    );
    const row = (name: string) => screen.getByRole("link", { name }).closest("tr") as HTMLElement;
    expect(row("mask ssn")).toHaveTextContent("Not enforced yet — no engine attached");
    expect(row("hash email")).not.toHaveTextContent("Not enforced");
    expect(vi.mocked(permApi.usePermRulesEnforcement).mock.calls.at(-1)?.[0]).toEqual({ ids: "r-off,r-on" });
  });

  test("a rule enforced in one catalog but not another says Partly enforced on its row, naming the catalog (live check D2)", async () => {
    const { AuthContext } = await import("@/auth/useAuth");
    serveRules([tagRule({ id: "r-part", name: "mask ssn" })]);
    vi.mocked(permApi.usePermRulesEnforcement).mockReturnValue({
      data: {
        status: 200,
        data: [
          {
            ruleId: "r-part",
            enforcedBy: 1,
            notEnforced: null,
            catalogs: [
              { catalogId: "c-a", catalogName: "sales", engines: [{ id: "e-1", name: "trino-a", engineType: "trino" }], notEnforced: null },
              { catalogId: "c-b", catalogName: "demo_warehouse", engines: [], notEnforced: "no_engine" },
            ],
          },
        ],
      },
    } as unknown as ReturnType<typeof permApi.usePermRulesEnforcement>);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <AuthContext.Provider value={{ me: { user_id: "u", email: "", platform_roles: [], permissions: ["policy:read"] }, isLoading: false }}>
          <MemoryRouter>
            <TagRulesSection tag={TAG} tagId="t-eu" />
          </MemoryRouter>
        </AuthContext.Provider>
      </QueryClientProvider>,
    );
    const row = screen.getByRole("link", { name: "mask ssn" }).closest("tr") as HTMLElement;
    expect(row).toHaveTextContent("Partly enforced. Not enforced in demo_warehouse — no engine attached.");
    expect(row).not.toHaveTextContent("Not enforced yet");
  });
});
