import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigationType } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { PersonRow, ReuseFinding, UnresolvedPrincipal, UsernameReuseReport } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import { expectStackedLabels } from "@/routes/governance/people/__tests__/stackedTable";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";

type Answer = { status: number; body: unknown } | "never";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/**
 * Stubs `fetch` for the two reads the index makes, through the real generated
 * client and `apiFetch`: `GET …/perm/people` (answered per call from its search
 * params) and `GET …/perm/unresolved-principals`. Returns the params of each
 * people call.
 */
function serve(
  people: (q: URLSearchParams) => Answer,
  unresolved: Answer = { status: 200, body: [] },
  reuse: Answer = { status: 403, body: { error: "forbidden" } },
) {
  const peopleCalls: URLSearchParams[] = [];
  let unresolvedCalls = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const reply = (a: Answer) => (a === "never" ? new Promise<Response>(() => {}) : Promise.resolve(json(a.status, a.body)));
      if (url.pathname === "/api/control-plane/perm/people") {
        peopleCalls.push(url.searchParams);
        return reply(people(url.searchParams));
      }
      if (url.pathname === "/api/control-plane/perm/unresolved-principals") {
        unresolvedCalls += 1;
        return reply(unresolved);
      }
      if (url.pathname === "/api/control-plane/perm/username-reuse") return reply(reuse);
      return Promise.resolve(json(404, { error: "not found" }));
    }),
  );
  return { peopleCalls, unresolvedCalls: () => unresolvedCalls };
}

const STEWARD: Me = { user_id: "s1", email: "s@example.com", platform_roles: [], permissions: ["policy:read"] };
const ADMIN: Me = { user_id: "a1", email: "a@example.com", platform_roles: ["platform_admin"], permissions: [] };

function Where() {
  const loc = useLocation();
  const type = useNavigationType();
  return (
    <output data-testid="where" data-nav={type}>
      {loc.pathname + loc.search}
    </output>
  );
}
const where = () => screen.getByTestId("where").textContent;

function open(url: string, me: Me = STEWARD) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/start", url]} initialIndex={1}>
            <Routes>
              <Route path="/governance/people/*" element={<PeopleRoutes />} />
              <Route path="*" element={null} />
            </Routes>
            <Where />
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

const user = (over: Partial<PersonRow> & { key: string; label: string }): PersonRow => ({
  kind: "user",
  detail: null,
  username: null,
  memberCount: null,
  withoutUsername: null,
  serviceAccount: false,
  ...over,
});

const USERS: PersonRow[] = [
  user({ key: "u-ana", label: "Ana Silva", detail: "ana@example.com", username: "ana" }),
  user({ key: "u-bo", label: "Bo Chen", detail: "bo@example.com", username: null }),
];

const page = (items: PersonRow[], total = items.length): Answer => ({ status: 200, body: { items, total } });

describe("People index: Users", () => {
  it("lists users with their email and username, each linking to its page", async () => {
    const calls = serve(() => page(USERS));
    open("/governance/people");
    expect(await screen.findByRole("heading", { level: 1, name: "People & groups" })).toBeInTheDocument();
    const ana = await screen.findByRole("link", { name: "Ana Silva" });
    expect(ana).toHaveAttribute("href", "/governance/people/users/u-ana");
    const anaRow = ana.closest("tr")!;
    expect(within(anaRow).getByText("ana@example.com")).toBeInTheDocument();
    expect(within(anaRow).getByText("ana")).toBeInTheDocument();
    const boRow = screen.getByRole("link", { name: "Bo Chen" }).closest("tr")!;
    expect(within(boRow).getByText("No username yet")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Users", selected: true })).toBeInTheDocument();
    expect(calls.peopleCalls[0].get("kind")).toBe("user");
    expect(calls.peopleCalls[0].get("includeServiceAccounts")).not.toBe("true");
  });

  it("shows service accounts only when asked, and marks them", async () => {
    const SA = user({ key: "u-sa", label: "etl-bot", detail: "service account", username: "etl_bot", serviceAccount: true });
    const calls = serve((q) => page(q.get("includeServiceAccounts") === "true" ? [...USERS, SA] : USERS));
    open("/governance/people");
    await screen.findByRole("link", { name: "Ana Silva" });
    expect(screen.queryByRole("link", { name: "etl-bot" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("switch", { name: "Show service accounts" }));
    const sa = await screen.findByRole("link", { name: "etl-bot" });
    expect(within(sa.closest("tr")!).getByText("Service account")).toBeInTheDocument();
    expect(calls.peopleCalls.at(-1)!.get("includeServiceAccounts")).toBe("true");
    expect(where()).toContain("serviceAccounts=1");
  });

  it("renders a skeleton while loading", async () => {
    serve(() => "never");
    open("/governance/people");
    expect(await screen.findByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("says when there are no users, never as an error", async () => {
    serve(() => page([]));
    open("/governance/people");
    expect(await screen.findByText("No users.")).toBeInTheDocument();
    expect(screen.getByText("Service accounts are hidden. Show service accounts lists them too.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Clear search" })).not.toBeInTheDocument();
  });

  it("shows the error with a retry, never an empty list", async () => {
    let fail = true;
    serve(() => (fail ? { status: 500, body: { error: "boom" } } : page(USERS)));
    open("/governance/people");
    expect(await screen.findByText("The users could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(screen.queryByText("No users.")).not.toBeInTheDocument();
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("link", { name: "Ana Silva" })).toBeInTheDocument();
  });
});

describe("People index: search and paging", () => {
  it("searches through ?q=, and a miss offers to clear it", async () => {
    const calls = serve((q) => page(q.get("q") ? [] : USERS));
    open("/governance/people?q=zz");
    expect(await screen.findByText("No user matches “zz”.")).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Search" })).toHaveValue("zz");
    expect(calls.peopleCalls[0].get("q")).toBe("zz");
    await userEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(await screen.findByRole("link", { name: "Ana Silva" })).toBeInTheDocument();
    expect(where()).toBe("/governance/people");
  });

  it("puts what is typed in ?q= and sends it", async () => {
    const calls = serve(() => page(USERS));
    open("/governance/people");
    await screen.findByRole("link", { name: "Ana Silva" });
    await userEvent.type(screen.getByRole("searchbox", { name: "Search" }), "an");
    await waitFor(() => expect(where()).toBe("/governance/people?q=an"));
    await waitFor(() => expect(calls.peopleCalls.at(-1)!.get("q")).toBe("an"));
  });

  it("pages through ?page=, 50 at a time", async () => {
    const many = Array.from({ length: 50 }, (_, i) => user({ key: `u-${i}`, label: `User ${i}` }));
    const calls = serve((q) =>
      page(q.get("offset") === "50" ? [user({ key: "u-last", label: "Last One" })] : many, 51),
    );
    open("/governance/people");
    expect(await screen.findByText("1–50 of 51")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(await screen.findByRole("link", { name: "Last One" })).toBeInTheDocument();
    expect(screen.getByText("51–51 of 51")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
    expect(where()).toBe("/governance/people?page=2");
    const last = calls.peopleCalls.at(-1)!;
    expect([last.get("limit"), last.get("offset")]).toEqual(["50", "50"]);
  });

  it("a new search starts again at the first page", async () => {
    serve(() => page(USERS, 120));
    open("/governance/people?page=3");
    await screen.findByRole("link", { name: "Ana Silva" });
    await userEvent.type(screen.getByRole("searchbox", { name: "Search" }), "a");
    await waitFor(() => expect(where()).toBe("/governance/people?q=a"));
  });
});

describe("People index: Groups and Roles", () => {
  const GROUPS: PersonRow[] = [
    { kind: "group", key: "eu/fin", label: "eu/fin", detail: "oidc", username: null, memberCount: 3, withoutUsername: 1, serviceAccount: false },
    { kind: "group", key: "solo", label: "solo", detail: "local", username: null, memberCount: 1, withoutUsername: 0, serviceAccount: false },
  ];
  const ROLES: PersonRow[] = [
    { kind: "role", key: "steward", label: "steward", detail: "platform, workspace", username: null, memberCount: 3, withoutUsername: 1, serviceAccount: false },
    { kind: "role", key: "auditor", label: "auditor", detail: "platform", username: null, memberCount: 1, withoutUsername: 0, serviceAccount: false },
  ];

  it("selects a tab through ?kind=, starting again at the first page", async () => {
    const calls = serve((q) => page(q.get("kind") === "group" ? GROUPS : USERS));
    open("/governance/people?page=2");
    await screen.findByRole("tab", { name: "Users", selected: true });
    await userEvent.click(screen.getByRole("tab", { name: "Groups" }));
    expect(await screen.findByRole("link", { name: "eu/fin" })).toBeInTheDocument();
    expect(where()).toBe("/governance/people?kind=group");
    expect(calls.peopleCalls.at(-1)!.get("kind")).toBe("group");
  });

  it("lists groups with their source and members, and how many have no username", async () => {
    serve(() => page(GROUPS));
    open("/governance/people?kind=group");
    const fin = await screen.findByRole("link", { name: "eu/fin" });
    expect(fin).toHaveAttribute("href", "/governance/people/groups/eu%2Ffin");
    const finRow = fin.closest("tr")!;
    expect(within(finRow).getByText("OIDC")).toBeInTheDocument();
    expect(within(finRow).getByText("3 members (1 with no username yet)")).toBeInTheDocument();
    const soloRow = screen.getByRole("link", { name: "solo" }).closest("tr")!;
    expect(within(soloRow).getByText("Local")).toBeInTheDocument();
    expect(within(soloRow).getByText("1 member")).toBeInTheDocument();
    expect(
      screen.getByText("A member with no username gets one when a grant naming the group is compiled."),
    ).toBeInTheDocument();
  });

  it("lists roles with their scopes and holders, and says who engines do not name", async () => {
    serve(() => page(ROLES));
    open("/governance/people?kind=role");
    const steward = await screen.findByRole("link", { name: "steward" });
    expect(steward).toHaveAttribute("href", "/governance/people/roles/steward");
    const row = steward.closest("tr")!;
    expect(within(row).getByText("platform, workspace")).toBeInTheDocument();
    expect(within(row).getByText("3 holders (1 not on engines: no username)")).toBeInTheDocument();
    expect(within(screen.getByRole("link", { name: "auditor" }).closest("tr")!).getByText("1 holder")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Holders are users. A holder with no username gets one when a grant naming the role is compiled; a group holding a role is not counted, as engines do not apply it.",
      ),
    ).toBeInTheDocument();
  });

  it("a group search that matches nothing names the group", async () => {
    serve(() => page([]));
    open("/governance/people?kind=role&q=x");
    expect(await screen.findByText("No role matches “x”.")).toBeInTheDocument();
  });
});

describe("People index: Unmatched names", () => {
  const row = (over: Partial<UnresolvedPrincipal> & { id: string; ref: string }): UnresolvedPrincipal => ({
    source: "grant",
    name: null,
    kind: "user",
    position: "include",
    effect: "allow",
    status: "unresolved",
    reason: "No user has this username.",
    failsOpen: false,
    ...over,
  });
  // Fail-closed first on purpose: the page puts fail-open rows first.
  const ROWS: UnresolvedPrincipal[] = [
    row({ id: "g-closed", ref: "old_ana" }),
    row({ id: "g-empty", ref: "nobody", kind: "group", status: "empty", reason: "The group nobody has no members." }),
    row({
      id: "r-open",
      source: "rule",
      name: "mask emails",
      ref: "gone_role",
      kind: "role",
      position: "exclude",
      effect: null,
      reason: "No role is named gone_role.",
      failsOpen: true,
    }),
    row({ id: "g-open", ref: "ex_user", effect: "deny", reason: "No user has this username.", failsOpen: true }),
  ];

  it("lists each stored name that matches nobody, fail-open first, with its reason and its grant or rule", async () => {
    serve(() => page([]), { status: 200, body: ROWS });
    open("/governance/people?kind=unmatched");
    expect(await screen.findByRole("tab", { name: "Unmatched names", selected: true })).toBeInTheDocument();
    const table = await screen.findByRole("table");
    const names = within(table)
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell")[0].textContent);
    expect(names).toEqual(["role gone_role", "user ex_user", "user old_ana", "group nobody"]);
    expectStackedLabels(table);

    const open1 = screen.getByText("gone_role").closest("tr")!;
    expect(within(open1).getByText("Widens access")).toBeInTheDocument();
    expect(within(open1).getByText("No role is named gone_role.")).toBeInTheDocument();
    expect(within(open1).getByRole("link", { name: "Rule mask emails" })).toHaveAttribute(
      "href",
      "/governance/rules/r-open",
    );
    expect(within(open1).getByText("Exemption, exclude")).toBeInTheDocument();
    const open2 = screen.getByText("ex_user").closest("tr")!;
    expect(within(open2).getByRole("link", { name: "Grant" })).toHaveAttribute("href", "/governance/grants/g-open");
    expect(within(open2).getByText("Deny, include")).toBeInTheDocument();
    const closed = screen.getByText("old_ana").closest("tr")!;
    expect(within(closed).getByText("Narrows access")).toBeInTheDocument();
    const empty = screen.getByText("nobody").closest("tr")!;
    expect(within(empty).getByText("Empty group")).toBeInTheDocument();
    expect(within(empty).getByText("The group nobody has no members.")).toBeInTheDocument();
  });

  it("filters by ?q= on the stored name, and a miss offers to clear it", async () => {
    serve(() => page([]), { status: 200, body: ROWS });
    open("/governance/people?kind=unmatched&q=ana");
    expect(await screen.findByText("old_ana")).toBeInTheDocument();
    expect(screen.queryByText("ex_user")).not.toBeInTheDocument();
    await userEvent.clear(screen.getByRole("searchbox", { name: "Search" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Search" }), "zz");
    expect(await screen.findByText("No unmatched name matches “zz”.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(await screen.findByText("ex_user")).toBeInTheDocument();
  });

  it("says when every stored name matches someone", async () => {
    serve(() => page([]), { status: 200, body: [] });
    open("/governance/people?kind=unmatched");
    expect(await screen.findByText("Every grant principal and rule exemption names someone egret knows.")).toBeInTheDocument();
  });

  it("shows the error with a retry", async () => {
    serve(() => page([]), { status: 500, body: { error: "boom" } });
    open("/governance/people?kind=unmatched");
    expect(
      await screen.findByText("The unmatched names could not be loaded. Retry, or reload the page."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("People index: Reused names", () => {
  const finding = (over: Partial<ReuseFinding> & Pick<ReuseFinding, "kind" | "verdict" | "username" | "place">): ReuseFinding => ({
    holder: null,
    retired: null,
    evidenceAt: null,
    evidence: "",
    ...over,
  });
  const ANA = { id: "u-ana2", email: "ana@new.example", createdAt: "2026-09-01T00:00:00Z", mintedAt: null };
  // Dead first and definite last on purpose: the tab puts definite findings first.
  const FINDINGS: ReuseFinding[] = [
    finding({
      kind: "holderless_ref",
      verdict: "dead",
      username: "gone",
      place: { type: "grant", id: "g-gone", position: "include", effect: "deny" },
      retired: { retiredAt: "2026-08-01T00:00:00Z", mintedFor: null, failsOpen: true },
      evidence: "nobody holds gone",
    }),
    finding({
      kind: "ref_predates_holder",
      verdict: "possible",
      username: "ana",
      place: { type: "engine_grant", id: "eg-1", engineId: "e-1" },
      holder: ANA,
      evidenceAt: "2026-07-02T00:00:00Z",
      evidence: "engine grant created before ana@new.example",
    }),
    finding({
      kind: "policy_predates_holder",
      verdict: "possible",
      username: "ana",
      place: { type: "policy", policyId: 7, name: "hand written", service: "trino prod", managedBy: "operator" },
      holder: ANA,
      evidenceAt: "2026-07-03T00:00:00Z",
      evidence: "policy last changed before ana@new.example",
    }),
    finding({
      kind: "admin_policy_named_non_admin",
      verdict: "stale",
      username: "cy",
      place: { type: "policy", policyId: 9, name: "egret-admin - catalog", service: "trino prod", managedBy: "infra" },
      holder: { id: "u-cy", email: "cy@corp.example", createdAt: "2026-01-01T00:00:00Z", mintedAt: null },
      evidenceAt: "2026-10-01T00:00:00Z",
      evidence: "the egret-admin body still gives engine ALL access to someone who is no longer an active platform admin",
    }),
    finding({
      kind: "ref_predates_holder",
      verdict: "definite",
      username: "bob",
      place: { type: "rule", id: "r-bob", position: "exclude" },
      holder: { id: "u-bob2", email: "bob@new.example", createdAt: "2026-09-02T00:00:00Z", mintedAt: "2026-09-02T00:00:00Z" },
      evidenceAt: "2026-07-01T09:00:00Z",
      evidence: "governance event perm_rule:create named user:bob",
    }),
  ];
  const REPORT: UsernameReuseReport = { generatedAt: "2026-10-23T10:00:00Z", findings: FINDINGS, baselineTakenAt: null };

  it("lists each finding, definite first, with its verdict, place, holder and evidence time", async () => {
    serve(() => page([]), undefined, { status: 200, body: REPORT });
    open("/governance/people?kind=reused", ADMIN);
    expect(await screen.findByRole("tab", { name: "Reused names", selected: true })).toBeInTheDocument();
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["bob", "ana", "ana", "cy", "gone"]);
    expectStackedLabels(table);

    const [bob, engineGrant, policy, stale, gone] = rows;
    expect(within(bob).getByText("Definite")).toBeInTheDocument();
    expect(within(bob).getByRole("link", { name: "Rule" })).toHaveAttribute("href", "/governance/rules/r-bob");
    expect(within(bob).getByRole("link", { name: "bob@new.example" })).toHaveAttribute(
      "href",
      "/governance/people/users/u-bob2",
    );
    expect(bob.querySelector("time")).toHaveAttribute("datetime", "2026-07-01T09:00:00Z");
    expect(within(bob).getByText("governance event perm_rule:create named user:bob")).toBeInTheDocument();

    expect(within(engineGrant).getByText("Possible")).toBeInTheDocument();
    expect(within(engineGrant).getByRole("link", { name: "Engine grant" })).toHaveAttribute(
      "href",
      "/governance/engines/e-1?tab=engine-permissions&grant=eg-1",
    );
    expect(within(policy).getByRole("link", { name: "Policy hand written on trino prod" })).toHaveAttribute(
      "href",
      "/governance/engines/profiles/trino%20prod?tab=policies",
    );

    expect(within(stale).getByText("Stale")).toBeInTheDocument();

    expect(within(gone).getByText("Dead")).toBeInTheDocument();
    expect(within(gone).getByText("Widens access")).toBeInTheDocument();
    expect(within(gone).getByText("Nobody")).toBeInTheDocument();
    expect(within(gone).getByRole("link", { name: "Grant" })).toHaveAttribute("href", "/governance/grants/g-gone");
  });

  it("filters by ?q= on the username", async () => {
    serve(() => page([]), undefined, { status: 200, body: REPORT });
    open("/governance/people?kind=reused&q=bo", ADMIN);
    expect(await screen.findByText("bob")).toBeInTheDocument();
    expect(screen.queryByText("gone")).not.toBeInTheDocument();
  });

  it("says when no name is reused, and what it could not check", async () => {
    serve(() => page([]), undefined, { status: 200, body: { ...REPORT, findings: [] } });
    open("/governance/people?kind=reused", ADMIN);
    expect(await screen.findByText("No reused engine usernames found.")).toBeInTheDocument();
    expect(screen.getByText(/History past audit retention cannot be checked\./)).toBeInTheDocument();
  });

  it("shows a refused read as an error with a retry", async () => {
    serve(() => page([]), undefined, { status: 403, body: { error: "forbidden" } });
    open("/governance/people?kind=reused", ADMIN);
    expect(await screen.findByText("The reused names could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("is not offered to a reader who is not a platform admin", async () => {
    serve(() => page(USERS), undefined, { status: 200, body: REPORT });
    open("/governance/people?kind=reused");
    expect(await screen.findByRole("tab", { name: "Users", selected: true })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Reused names" })).not.toBeInTheDocument();
  });
});

describe("ByNameRedirect", () => {
  it("replaces /users/by-name/:username with the user's page, keeping the tab", async () => {
    const calls = serve((q) => page(q.get("username") === "ana" ? [USERS[0]] : []));
    open("/governance/people/users/by-name/ana?tab=permissions");
    await waitFor(() => expect(where()).toBe("/governance/people/users/u-ana?tab=permissions"));
    expect(screen.getByTestId("where")).toHaveAttribute("data-nav", "REPLACE");
    const asked = calls.peopleCalls[0];
    expect([asked.get("kind"), asked.get("username")]).toEqual(["user", "ana"]);
  });

  // Final review I2: the route matches citext; engines compare byte for byte, so `Ana` is not ana.
  it("does not open a user whose username matches only case-insensitively, and says why", async () => {
    serve((q) => page(q.get("username")?.toLowerCase() === "ana" ? [USERS[0]] : []));
    open("/governance/people/users/by-name/Ana?tab=permissions");
    expect(
      await screen.findByText(
        "“Ana” matches the user ana only case-insensitively; engines compare names byte for byte, so this name matches nobody.",
      ),
    ).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/by-name/Ana?tab=permissions");
    expect(screen.getByRole("link", { name: "Unmatched names" })).toHaveAttribute("href", "/governance/people?kind=unmatched");
    expect(screen.getByRole("link", { name: "ana's page" })).toHaveAttribute("href", "/governance/people/users/u-ana");
  });

  it("says when nobody has the username", async () => {
    serve(() => page([]));
    open("/governance/people/users/by-name/ana");
    expect(await screen.findByText("No user has the username ana.")).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/by-name/ana");
  });

  it("shows the error with a retry", async () => {
    serve(() => ({ status: 500, body: { error: "boom" } }));
    open("/governance/people/users/by-name/ana");
    expect(await screen.findByText("The user could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
  });
});
