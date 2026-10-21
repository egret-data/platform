import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import { getControlPlaneTagsListTagsQueryKey } from "@/api/generated/control-plane-tags/control-plane-tags";
import type { BrowseResponse, ExplainResponse, GrantView, ObjectTags, RuleView, Scope } from "@/api/generated/model";
import { tagsQueryKey } from "@/api/tags";
import {
  getControlPlaneGovernanceObjectHandwrittenQueryKey,
  getControlPlaneGovernanceTagUsageQueryKey,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { browseItems, catalog, serveData, type FakeData } from "./fakeDataServer";
import { handwrittenAnswer } from "./handwrittenFixture";

/**
 * The column panel (`?column=`), the "Who can read?" dialog and the "Add tag"
 * dialog, through the real page and the real generated client (spec §2.2,
 * F9, F10).
 */

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const ICE = { ...catalog("c-ice", "ice", "iceberg", true), engines: ["trino-1"] };
const PG = { ...catalog("c-pg", "pg", "hive"), engines: ["trino-2"] };

const ORDERS_COLUMNS: BrowseResponse = {
  source: { kind: "egret" },
  engines: [],
  columns: [
    { name: "id", type: "long" },
    { name: "email", type: "string" },
    { name: "amount", type: "decimal(10,2)" },
  ],
};

function tag(key: string) {
  return {
    id: `t-${key}`,
    key,
    name: key.toUpperCase(),
    description: "",
    color: "#336699",
    assignment_count: 0,
    created_at: "2026-10-01T00:00:00Z",
  };
}

const ORDERS_TAGS: ObjectTags = {
  supported: true,
  assignments: [
    { schema: "sales", table: "orders", column: "", tag: tag("confidential") },
    // Stored in another case: column names compare without regard to case.
    { schema: "sales", table: "orders", column: "EMAIL", tag: tag("pii") },
    { schema: "sales", table: "orders", column: "amount", tag: tag("finance") },
  ],
};

const ok = (body: unknown) => ({ status: 200, body });
const boom = { status: 500, body: { error: "internal error" } };

function pathRule(id: string, scope: Omit<Scope, "catalog_id">, kind: RuleView["kind"]): RuleView {
  return {
    id,
    name: id,
    kind,
    selector: { type: "path", catalog_id: "c-ice", ...scope },
    exempt: { include: [], exclude: [] },
    catalog_name: "ice",
  };
}
function tagRule(id: string, key: string, kind: RuleView["kind"]): RuleView {
  return { id, name: id, kind, selector: { type: "tag", key }, exempt: { include: [], exclude: [] }, tag_name: key.toUpperCase() };
}
const HASH = { type: "mask", mask_type: "MASK_HASH" } as const;
const REDACT = { type: "mask", mask_type: "MASK" } as const;
const US_ONLY = { type: "row_filter", filter_expr: "region = 'us'" } as const;

const RULES: RuleView[] = [
  // Unsorted on purpose.
  tagRule("redact_pii", "pii", REDACT),
  pathRule("hash_amount", { schema: "sales", table: "orders", column: "amount" }, HASH),
  { ...pathRule("hash_email", { schema: "sales", table: "orders", column: "email" }, HASH), exempt: { include: [{ kind: "group", ref: "auditors" }], exclude: [] } },
  tagRule("redact_confidential", "confidential", REDACT),
  tagRule("redact_finance", "finance", REDACT),
  // A row filter is not a mask.
  pathRule("us_only", { schema: "sales", table: "orders" }, US_ONLY),
];

function g(id: string, scope: Omit<Scope, "catalog_id">, over: Partial<GrantView> = {}): GrantView {
  return {
    id,
    effect: "allow",
    actions: ["read"],
    scope: { catalog_id: "c-ice", ...scope },
    principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    origin: { type: "explicit" },
    catalog_name: "ice",
    ...over,
  };
}
const GRANTS: GrantView[] = [
  g("g-table", { schema: "sales", table: "orders" }),
  g("g-email", { schema: "sales", table: "orders", column: "email" }, { effect: "deny" }),
  g("g-amount", { schema: "sales", table: "orders", column: "amount" }),
  g("g-pattern", { schema: "sales", table: "orders", column: "e*" }, { principals: { include: [{ kind: "user", ref: "bob" }], exclude: [] } }),
];

const EXPLAIN: ExplainResponse = {
  outcome: "per_profile",
  profiles: [
    {
      service_id: 11,
      service_name: "egret-trino-prod",
      engine: "trino",
      tags: [],
      tag_policies_in_download: 0,
      accesses: [
        {
          access_type: "select",
          decision: { kind: "allowed", policy_id: 7, policy_name: "perm-grant-1" },
          attribution: { kind: "grant", grant_id: "g-table" },
        },
      ],
    },
  ],
};

function fixture(over: Partial<FakeData> = {}): FakeData {
  return {
    catalogs: ok([PG, ICE]),
    browse: {
      "c-ice": ok(browseItems([{ name: "sales", kind: "schema" }])),
      "c-ice|sales": ok(browseItems([{ name: "orders", kind: "table" }])),
      "c-ice|sales|orders": ok(ORDERS_COLUMNS),
      "c-pg": ok(browseItems([{ name: "s", kind: "schema" }])),
      "c-pg|s": ok(browseItems([{ name: "t", kind: "table" }])),
      "c-pg|s|t": ok(ORDERS_COLUMNS),
    },
    tags: { "c-ice|sales|orders": ok(ORDERS_TAGS) },
    rules: ok(RULES),
    grants: ok(GRANTS),
    explain: ok(EXPLAIN),
    tagCatalogue: ok([tag("pii"), tag("confidential"), tag("secret"), tag("finance")]),
    ...over,
  };
}

const STEWARD: Me = { user_id: "u1", email: "s@egret.local", platform_roles: [], permissions: ["policy:read"] };
const TAGGER: Me = { ...STEWARD, permissions: ["policy:read", "catalog_registry:update", "tag:read"] };

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

function renderPage(url: string, me: Me | null = STEWARD, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[url]}>
            <Routes>
              <Route
                path="/governance/data/*"
                element={
                  <>
                    <DataRoutes />
                    <Where />
                  </>
                }
              />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const where = () => screen.getByTestId("where").textContent;
const items = (el: HTMLElement, name: string) =>
  within(within(el).getByRole("list", { name })).getAllByRole("listitem").map((li) => li.textContent);

afterEach(() => {
  vi.unstubAllGlobals();
  toast.success.mockClear();
  toast.error.mockClear();
});

describe("ColumnPanel", () => {
  it("opens from ?column= with the column's type, its own tags and the table's marked from table", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    expect(within(panel).getByText("ice.sales.orders.email")).toBeInTheDocument();
    expect(await within(panel).findByText("string")).toBeInTheDocument();
    await waitFor(() => expect(items(panel, "Classifications of email")).toEqual(["PII", "CONFIDENTIAL from table"]));
  });

  it("lists the masking rules that select the column, by path or by its own or its table's tag, with exemptions", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() =>
      expect(items(panel, "Rules of email")).toEqual([
        "hash_email Mask · Hash · by path · except group auditors",
        "redact_confidential Mask · Redact · by classification CONFIDENTIAL",
        "redact_pii Mask · Redact · by classification PII",
      ]),
    );
    // F10: which rules select the column, never who sees what.
    expect(within(panel).queryByText(/can see|cannot see/i)).not.toBeInTheDocument();
    expect(within(panel).getByRole("heading", { name: "Classifications" })).toBeInTheDocument();
    expect(within(panel).getByRole("heading", { name: "Rules" })).toBeInTheDocument();
    for (const retired of ["Tags", "Masking rules"]) {
      expect(within(panel).queryByRole("heading", { name: retired })).not.toBeInTheDocument();
    }
  });

  it("names a workspace exemption by the workspace's name", async () => {
    const withWorkspace = { ...ICE, workspaces: [{ id: "w1", name: "Finance", slug: "finance" }] };
    const exempted = { ...pathRule("hash_email", { schema: "sales", table: "orders", column: "email" }, HASH), exempt: { include: [{ kind: "workspace_members" as const, ref: "w1" }], exclude: [] } };
    serveData(fixture({ catalogs: ok([PG, withWorkspace]), rules: ok([exempted]) }));
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() => expect(items(panel, "Rules of email")).toEqual(["hash_email Mask · Hash · by path · except members of workspace Finance"]));
  });

  it("lists the grants set on the column, a pattern one marked, deny marked, and none from other columns", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() =>
      expect(items(panel, "Grants on email")).toEqual([
        "role analyst cannot read ice.sales.orders.emailDeny",
        "user bob can read ice.sales.orders.“e*”Allow · matches “e*”",
      ]),
    );
    expect(within(panel).getByText("Grants on the table and above reach this column too. See Permissions.")).toBeInTheDocument();
    // #4b: hand-written policies are listed below; explain gives the engines' answer.
    expect(
      within(panel).getByText(
        "egret grants are above; manual policies below. To see who can actually read, use Who can read this column?",
      ),
    ).toBeInTheDocument();
  });

  it("says when nothing selects the column and no grant is set on it", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=id");
    const panel = await screen.findByRole("dialog", { name: "id" });
    expect(await within(panel).findByText("No egret grant is set on this column")).toBeInTheDocument();
    expect(within(panel).getByText(/^egret grants are above; manual policies below\./)).toBeInTheDocument();
    // The table's tag still reaches it, and so does the rule on that tag.
    await waitFor(() => expect(items(panel, "Rules of id")).toEqual(["redact_confidential Mask · Redact · by classification CONFIDENTIAL"]));
    expect(items(panel, "Classifications of id")).toEqual(["CONFIDENTIAL from table"]);
  });

  it("says the grants and rules could not be loaded, never that there are none", async () => {
    serveData(fixture({ grants: boom, rules: boom }));
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    expect(await within(panel).findByText("The grants could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel).getByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel).queryByText("No egret grant is set on this column")).not.toBeInTheDocument();
  });

  it("tells a viewer without policy:read why, and never asks for grants or rules", async () => {
    const { permCalls } = serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email", { ...STEWARD, permissions: [] });
    const panel = await screen.findByRole("dialog", { name: "email" });
    expect(await within(panel).findByText("Reading grants and rules needs the policy:read permission.")).toBeInTheDocument();
    await waitFor(() => expect(items(panel, "Classifications of email")).toEqual(["PII", "CONFIDENTIAL from table"]));
    expect(permCalls).toEqual([]);
  });

  it("opens from a column's name in the Overview", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    await user.click(await screen.findByRole("link", { name: "amount" }));
    expect(await screen.findByRole("dialog", { name: "amount" })).toBeInTheDocument();
    expect(where()).toBe("/governance/data/ice/sales/orders?column=amount");
  });

  it("traps focus, and Escape closes it and removes ?column= but keeps ?tab=", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions&column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() => expect(panel.contains(document.activeElement)).toBe(true));
    for (let i = 0; i < 8; i++) {
      await user.tab();
      expect(panel.contains(document.activeElement)).toBe(true);
    }
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(where()).toBe("/governance/data/ice/sales/orders?tab=permissions");
  });

  // Live check D6: Escape left focus on <body>, not on the link that opened the panel.
  it("returns focus to the column's link on Escape, even after focus moved inside the panel", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    const link = await screen.findByRole("link", { name: "email" });
    link.focus();
    await user.keyboard("{Enter}");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() => expect(panel.contains(document.activeElement)).toBe(true));
    await user.click(await within(panel).findByRole("button", { name: "Who can read this column?" }));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.getAllByRole("dialog")).toHaveLength(1));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("link", { name: "email" })).toHaveFocus();
  });

  it("returns focus to the column's link on Escape when the panel opened from the URL", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await screen.findByRole("link", { name: "email", hidden: true });
    await waitFor(() => expect(panel.contains(document.activeElement)).toBe(true));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("link", { name: "email" })).toHaveFocus();
  });

  it("returns focus to the page's heading on Escape when no tab lists the column", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions&column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    await waitFor(() => expect(panel.contains(document.activeElement)).toBe(true));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { level: 1, name: "orders" })).toHaveFocus();
  });

  it("says a column the table does not have is not there, and offers nothing for it", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=nope", TAGGER);
    const panel = await screen.findByRole("dialog", { name: "nope" });
    expect(await within(panel).findByText("No column named “nope” in ice.sales.orders.")).toBeInTheDocument();
    expect(within(panel).getAllByRole("button").map((b) => b.textContent)).toEqual(["Close"]);
    expect(within(panel).queryByText(/No classification|No rule|No grant/)).not.toBeInTheDocument();
  });

  it("says the column could not be checked when the table's columns cannot be listed, and claims nothing", async () => {
    for (const answer of [boom, ok({ source: null, engines: [], notListed: { reason: "engine_error", detail: "The engine refused." } })]) {
      serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales|orders": answer } }));
      const { unmount } = renderPage("/governance/data/ice/sales/orders?column=email", TAGGER);
      const panel = await screen.findByRole("dialog", { name: "email" });
      expect(
        await within(panel).findByText("“email” could not be checked: the columns of ice.sales.orders could not be listed."),
      ).toBeInTheDocument();
      expect(within(panel).queryByRole("button", { name: "Add classification" })).not.toBeInTheDocument();
      expect(within(panel).queryByRole("button", { name: "Who can read this column?" })).not.toBeInTheDocument();
      expect(within(panel).queryByText(/No classification|No rule|No grant/)).not.toBeInTheDocument();
      unmount();
      vi.unstubAllGlobals();
    }
  });

  it("names the column as the source spells it, whatever case the URL uses", async () => {
    const { tagWrites } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=EMAIL", TAGGER);
    const panel = await screen.findByRole("dialog", { name: "email" });
    expect(within(panel).getByText("ice.sales.orders.email")).toBeInTheDocument();
    await user.click(await within(panel).findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    await user.click(await screen.findByRole("option", { name: "SECRET" }));
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));
    await waitFor(() => expect(tagWrites).toHaveLength(1));
    expect(tagWrites[0].body).toEqual({ schema: "sales", table: "orders", column: "email", tag_id: "t-secret" });
  });

  it("opens no panel above the object level", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales?column=email");
    expect(await screen.findByRole("heading", { level: 1, name: "sales" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("Who can read?", () => {
  it("explains read for exactly this column, and renders the verdict per profile", async () => {
    const { permCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email");
    await user.click(await within(await screen.findByRole("dialog", { name: "email" })).findByRole("button", { name: "Who can read this column?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    const explain = within(dialog).getByRole("button", { name: "Explain" });
    expect(explain).toBeDisabled();
    expect(within(dialog).getByText("Explain needs a user.")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("User"), "alice");
    await user.click(explain);

    const result = await within(dialog).findByTestId("explain-result");
    const profile = within(result).getByTestId("explain-profile");
    expect(within(profile).getByText("egret-trino-prod")).toBeInTheDocument();
    expect(within(profile).getByText("Trino")).toBeInTheDocument();
    expect(permCalls.find((c) => c.call === "POST /perm/explain")?.body).toEqual({
      subject: { user: "alice" },
      action: "read",
      target: { catalog_id: "c-ice", schema: "sales", table: "orders", column: "email" },
    });
  });

  it("explains the object itself from the header", async () => {
    const { permCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    await user.click(await screen.findByRole("button", { name: "Who can read?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    expect(within(dialog).getByText("ice.sales.orders")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("User"), "alice");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    await within(dialog).findByTestId("explain-result");
    expect(permCalls.find((c) => c.call === "POST /perm/explain")?.body).toEqual({
      subject: { user: "alice" },
      action: "read",
      target: { catalog_id: "c-ice", schema: "sales", table: "orders" },
    });
  });

  it("explains a catalog with only the catalog set", async () => {
    const { permCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice");
    await user.click(await screen.findByRole("button", { name: "Who can read?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "alice");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    await within(dialog).findByTestId("explain-result");
    expect(permCalls.find((c) => c.call === "POST /perm/explain")?.body).toMatchObject({ target: { catalog_id: "c-ice" } });
  });

  it("shows the server's refusal and keeps the user typed", async () => {
    serveData(fixture({ explain: { status: 500, body: { error: "The policy artifact could not be built." } } }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    await user.click(await screen.findByRole("button", { name: "Who can read?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "alice");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    expect(await within(dialog).findByText("The policy artifact could not be built.")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("User")).toHaveValue("alice");
  });

  it("never shows an answer, or a refusal, under a user it was not asked for", async () => {
    let answer: (a: { status: number; body: unknown }) => void = () => {};
    serveData(fixture({ explain: () => new Promise((resolve) => (answer = resolve)) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    await user.click(await screen.findByRole("button", { name: "Who can read?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    const field = within(dialog).getByLabelText("User");
    await user.type(field, "alice");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    // The answer for alice arrives after the field names someone else.
    await user.type(field, "2");
    answer(ok(EXPLAIN));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Explain" })).toBeEnabled());
    expect(within(dialog).queryByTestId("explain-result")).not.toBeInTheDocument();

    // A refusal for one user is gone once the field names another.
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    answer({ status: 500, body: { error: "The policy artifact could not be built." } });
    expect(await within(dialog).findByText("The policy artifact could not be built.")).toBeInTheDocument();
    await user.type(field, "3");
    expect(within(dialog).queryByText("The policy artifact could not be built.")).not.toBeInTheDocument();
  });

  it("offers a user, a group or a role, and asks a role's members", async () => {
    const members = {
      membersTotal: 2,
      limit: 200,
      truncated: false,
      evaluated: [{ userId: "u-a", label: "Alice", username: "alice", result: EXPLAIN }],
      notEvaluated: [{ userId: "u-b", label: "Bea", reason: "no_username" }],
    };
    const { permCalls } = serveData(fixture({ explainPrincipal: ok(members) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    await user.click(await screen.findByRole("button", { name: "Who can read?" }));
    const dialog = await screen.findByRole("dialog", { name: "Who can read?" });
    expect(dialog).toHaveTextContent("Whether a user, or each member of a group or role, can read this, per policy profile");

    await user.click(within(dialog).getByRole("combobox", { name: "Subject" }));
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual(["User", "Group", "Role"]);
    await user.click(screen.getByRole("option", { name: "Role" }));
    expect(within(dialog).getByText("Explain needs a role.")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Role"), "reader");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const view = await within(dialog).findByTestId("members-explain");
    expect(within(view).getByRole("region", { name: "Not asked" })).toHaveTextContent("Bea");
    expect(permCalls.find((c) => c.call === "POST /perm/explain/principal")?.body).toEqual({
      principal: { kind: "role", ref: "reader" },
      action: "read",
      target: { catalog_id: "c-ice", schema: "sales", table: "orders" },
    });
    expect(permCalls.some((c) => c.call === "POST /perm/explain")).toBe(false);

    // Another kind is another subject: the role's answer is gone.
    await user.click(within(dialog).getByRole("combobox", { name: "Subject" }));
    await user.click(await screen.findByRole("option", { name: "Group" }));
    expect(within(dialog).queryByTestId("members-explain")).not.toBeInTheDocument();
  });

  it("is not offered without policy:read", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email", { ...STEWARD, permissions: ["catalog_registry:update"] });
    const panel = await screen.findByRole("dialog", { name: "email" });
    expect(within(panel).queryByRole("button", { name: "Who can read this column?" })).not.toBeInTheDocument();
  });
});

describe("Add classification", () => {
  it("adds a tag to the column through resource-tags, keyed by the warehouse, then refetches and confirms", async () => {
    const { tagWrites, tagCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email", TAGGER);
    const panel = await screen.findByRole("dialog", { name: "email" });
    await user.click(await within(panel).findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    expect(within(dialog).getByText("ice.sales.orders.email")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Attach" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "Attach" })).toHaveAccessibleDescription("Choose a classification to attach.");
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    // PII is already on the column; the table's tag is not the column's own.
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual(["CONFIDENTIAL", "FINANCE", "SECRET"]);
    await user.click(screen.getByRole("option", { name: "SECRET" }));
    const before = tagCalls.length;
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add classification" })).not.toBeInTheDocument());
    expect(tagWrites).toEqual([
      { warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "email", tag_id: "t-secret" } },
    ]);
    expect(toast.success).toHaveBeenCalledWith("Classification attached.");
    await waitFor(() => expect(tagCalls.length).toBeGreaterThan(before));
  });

  // M4: the Classification pages, RuleDialog and the rule lists read the catalogue under `tagsQueryKey`.
  it("refetches the tag catalogue under both keys it is cached by", async () => {
    serveData(fixture());
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email", TAGGER, client);
    const panel = await screen.findByRole("dialog", { name: "email" });
    await user.click(await within(panel).findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    await user.click(await screen.findByRole("option", { name: "SECRET" }));
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Classification attached."));
    const keys = invalidate.mock.calls.map(([filters]) => filters?.queryKey);
    expect(keys).toContainEqual(tagsQueryKey);
    expect(keys).toContainEqual(getControlPlaneTagsListTagsQueryKey());
    // CL16: the tag's Where used (route C) is refreshed too.
    expect(keys).toContainEqual(getControlPlaneGovernanceTagUsageQueryKey("t-secret"));
    // And the Hand-written sections (route A), which match tag policies through the object's tags.
    const routeA = getControlPlaneGovernanceObjectHandwrittenQueryKey("c-ice", { schema: "sales", table: "orders" });
    expect(invalidate.mock.calls.some(([f]) => f?.predicate?.({ queryKey: routeA } as never) === true)).toBe(true);
  });

  // M5: the URL may differ in case from the source; the tag is written with the listing's spelling.
  it("adds a tag from the header with the table's source spelling, whatever case the URL uses", async () => {
    const base = fixture();
    const { tagWrites } = serveData(
      fixture({
        browse: { ...base.browse, "c-ice|sales|ORDERS": ok(ORDERS_COLUMNS) },
        tags: { "c-ice|sales|ORDERS": ok(ORDERS_TAGS) },
      }),
    );
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/ORDERS", TAGGER);
    await user.click(await screen.findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    await waitFor(() => expect(within(dialog).getByText("ice.sales.orders")).toBeInTheDocument());
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    await user.click(await screen.findByRole("option", { name: "PII" }));
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));
    await waitFor(() => expect(tagWrites).toHaveLength(1));
    expect(tagWrites[0]).toEqual({ warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "", tag_id: "t-pii" } });
  });

  it("adds a tag to the table from the header", async () => {
    const { tagWrites } = serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders", TAGGER);
    await user.click(await screen.findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    // CONFIDENTIAL is on the table already; a column's tag is not the table's.
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual(["FINANCE", "PII", "SECRET"]);
    await user.click(screen.getByRole("option", { name: "PII" }));
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));
    await waitFor(() => expect(tagWrites).toHaveLength(1));
    expect(tagWrites[0]).toEqual({ warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "", tag_id: "t-pii" } });
  });

  it("shows the server's refusal beside the field and keeps the choice", async () => {
    serveData(fixture({ tagWrite: { status: 409, body: { error: "Two mask rules would decide sales.orders.email differently." } } }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email", TAGGER);
    await user.click(await within(await screen.findByRole("dialog", { name: "email" })).findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    await user.click(within(dialog).getByRole("combobox", { name: "Classification" }));
    await user.click(await screen.findByRole("option", { name: "SECRET" }));
    await user.click(within(dialog).getByRole("button", { name: "Attach" }));
    expect(await within(dialog).findByText("Two mask rules would decide sales.orders.email differently.")).toBeInTheDocument();
    expect(within(dialog).getByRole("combobox", { name: "Classification" })).toHaveTextContent("SECRET");
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("says the tag list could not be read without tag:read", async () => {
    serveData(fixture({ tagCatalogue: { status: 403, body: { error: "forbidden" } } }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders", { ...TAGGER, permissions: ["catalog_registry:update"] });
    await user.click(await screen.findByRole("button", { name: "Add classification" }));
    const dialog = await screen.findByRole("dialog", { name: "Add classification" });
    expect(await within(dialog).findByText("Reading the classification list needs the tag:read permission.")).toBeInTheDocument();
  });

  it("is disabled with the reason on a catalog that is not egret's, in the header and the panel (F9)", async () => {
    const { tagCalls } = serveData(fixture());
    renderPage("/governance/data/pg/s/t?column=email", TAGGER);
    const panel = await screen.findByRole("dialog", { name: "email" });
    const inPanel = await within(panel).findByRole("button", { name: "Add classification" });
    expect(inPanel).toBeDisabled();
    expect(inPanel).toHaveAccessibleDescription("Classifications can be attached to egret-managed catalogs only");
    expect(within(panel).queryByRole("list", { name: "Classifications of email" })).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
  });

  it("is disabled with the reason in the header of an object in a catalog that is not egret's (F9)", async () => {
    serveData(fixture());
    renderPage("/governance/data/pg/s/t", TAGGER);
    const button = await screen.findByRole("button", { name: "Add classification" });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription("Classifications can be attached to egret-managed catalogs only");
  });

  it("is not offered without catalog_registry:update", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?column=email", { ...STEWARD, permissions: ["policy:read", "tag:read"] });
    const panel = await screen.findByRole("dialog", { name: "email" });
    await within(panel).findByRole("list", { name: "Classifications of email" });
    expect(within(panel).queryByRole("button", { name: "Add classification" })).not.toBeInTheDocument();
  });

  it("is not offered on a catalog or a schema, which cannot be tagged", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales", TAGGER);
    expect(await screen.findByRole("button", { name: "Who can read?" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add classification" })).not.toBeInTheDocument();
  });
});

/** #4b spec §6 (F12): every hand-written policy that reaches the column, from route A with `column`. */
describe("the column panel's manual policies", () => {
  it("lists every row for the column, asking route A with the column", async () => {
    const calls = serveData(fixture({ handwritten: { "c-ice|sales|orders|email": handwrittenAnswer("c-ice") } }));
    renderPage("/governance/data/ice/sales/orders?column=email");
    const panel = await screen.findByRole("dialog", { name: "email" });
    const section = await within(panel).findByRole("region", { name: "Manual policies (3)" });
    for (const name of ["hw-access", "hw-mask", "hw-filter"]) {
      expect(within(section).getByRole("link", { name })).toBeInTheDocument();
    }
    expect(calls.handwrittenCalls).toContain("c-ice|sales|orders|email");
  });
});
