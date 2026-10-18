import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { GroupPrincipal, Membership, MemberRef, RolePrincipal, UserPrincipal } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";
import { expectStackedLabels } from "@/routes/governance/people/__tests__/stackedTable";

type Answer = { status: number; body: unknown } | "never";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** Stubs `fetch` for the three principal reads, keyed by the decoded path after `/perm/people/`. */
function serve(answers: Record<string, Answer>) {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const m = /^\/api\/control-plane\/perm\/people\/(users|groups|roles)\/([^/]+)$/.exec(url.pathname);
      if (!m) return Promise.resolve(json(404, { error: "not found" }));
      const key = `${m[1]}/${decodeURIComponent(m[2])}`;
      asked.push(key);
      const a = answers[key] ?? { status: 404, body: { error: "not found" } };
      return a === "never" ? new Promise<Response>(() => {}) : Promise.resolve(json(a.status, a.body));
    }),
  );
  return asked;
}

const me = (permissions: string[]): Me => ({ user_id: "s1", email: "s@example.com", platform_roles: [], permissions });
const STEWARD = me(["policy:read"]);

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}
const where = () => screen.getByTestId("where").textContent;

function open(url: string, as: Me = STEWARD) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[url]}>
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

const tabNames = () => screen.getAllByRole("tab").map((t) => t.textContent);

const membership = (over: Partial<Membership> & Pick<Membership, "via">): Membership => ({
  role: null,
  scope: null,
  expiresAt: null,
  expired: false,
  onEngines: true,
  notOnEngines: null,
  ...over,
});

const ANA: UserPrincipal = {
  id: "u-ana",
  email: "ana@example.com",
  name: "Ana Silva",
  username: "ana",
  status: "active",
  serviceAccount: false,
  groups: [membership({ via: { kind: "group", name: "analysts" } })],
  roles: [
    membership({
      via: { kind: "role", name: "steward" },
      role: "steward",
      scope: { type: "platform" },
      expiresAt: "2026-12-31T00:00:00Z",
    }),
    membership({
      via: { kind: "role", name: "auditor" },
      role: "auditor",
      scope: { type: "platform" },
      expiresAt: "2026-09-01T00:00:00Z",
      expired: true,
      onEngines: false,
      notOnEngines: "expired",
    }),
    membership({
      via: { kind: "group_role", group: "analysts", role: "reader" },
      role: "reader",
      scope: { type: "workspace", id: "w1", name: "Sales" },
      onEngines: false,
      notOnEngines: "held_through_group",
    }),
  ],
  workspaces: [
    membership({
      via: { kind: "group_workspace", group: "analysts", id: "w1", name: "Sales" },
      role: "reader",
      scope: { type: "workspace", id: "w1", name: "Sales" },
      onEngines: false,
      notOnEngines: "held_through_group",
    }),
  ],
  standsFor: [],
  clearances: [],
};

const member = (over: Partial<MemberRef> & Pick<MemberRef, "userId" | "label">): MemberRef => ({
  username: null,
  serviceAccount: false,
  onEngines: true,
  notOnEngines: null,
  ...over,
});

const ANALYSTS: GroupPrincipal = {
  name: "eu/analysts",
  source: "oidc",
  isPublic: false,
  members: [
    member({ userId: "u-ana", label: "Ana Silva", username: "ana" }),
    member({ userId: "u-bo", label: "Bo Chen", onEngines: false, notOnEngines: "no_username" }),
    member({ userId: "u-bot", label: "etl-bot", username: "etl_bot", serviceAccount: true }),
  ],
  serviceAccounts: 1,
  roles: [
    membership({
      via: { kind: "role", name: "reader" },
      role: "reader",
      scope: { type: "workspace", id: "w1", name: "Sales" },
      onEngines: false,
      notOnEngines: "held_through_group",
    }),
  ],
  workspaces: [
    membership({
      via: { kind: "workspace", id: "w1", name: "Sales" },
      role: "reader",
      scope: { type: "workspace", id: "w1", name: "Sales" },
      onEngines: false,
      notOnEngines: "held_through_group",
    }),
  ],
  standsFor: [],
  clearances: [],
};

const PUBLIC: GroupPrincipal = {
  name: "public",
  source: null,
  isPublic: true,
  members: [],
  serviceAccounts: 0,
  roles: [],
  workspaces: [],
  standsFor: [],
  clearances: [],
};

const STEWARD_ROLE: RolePrincipal = {
  name: "steward",
  scopes: ["platform", "workspace"],
  holders: [
    member({ userId: "u-ana", label: "Ana Silva", username: "ana" }),
    member({ userId: "u-bo", label: "Bo Chen", onEngines: false, notOnEngines: "no_username" }),
  ],
  serviceAccounts: 0,
  holderGroups: [{ name: "analysts", source: "local" }],
  standsFor: [],
  clearances: [],
};

describe("principal page: tabs per kind", () => {
  it.each<[string, string, Record<string, Answer>, string[]]>([
    ["user", "/governance/people/users/u-ana", { "users/u-ana": { status: 200, body: ANA } }, ["Overview", "Permissions", "Masks & filters", "Clearances", "Check access"]],
    ["group", "/governance/people/groups/eu%2Fanalysts", { "groups/eu/analysts": { status: 200, body: ANALYSTS } }, ["Overview", "Permissions", "Masks & filters", "Clearances", "Check access"]],
    ["role", "/governance/people/roles/steward", { "roles/steward": { status: 200, body: STEWARD_ROLE } }, ["Overview", "Permissions", "Clearances", "Check access"]],
    ["public", "/governance/people/groups/public", { "groups/public": { status: 200, body: PUBLIC } }, ["Permissions", "Masks & filters"]],
  ])("a %s has the tabs of spec §2", async (_kind, url, answers, tabs) => {
    serve(answers);
    open(url);
    await waitFor(() => expect(tabNames()).toEqual(tabs));
    expect(screen.getAllByRole("tab")[0]).toHaveAttribute("aria-selected", "true");
  });

  it("selects the tab ?tab= names", async () => {
    serve({ "roles/steward": { status: 200, body: STEWARD_ROLE } });
    open("/governance/people/roles/steward?tab=clearances");
    expect(await screen.findByRole("tab", { name: "Clearances", selected: true })).toBeInTheDocument();
  });

  it("a tab the kind does not have, or no tab at all, shows the first", async () => {
    serve({ "roles/steward": { status: 200, body: STEWARD_ROLE } });
    open("/governance/people/roles/steward?tab=masks");
    expect(await screen.findByRole("tab", { name: "Overview", selected: true })).toBeInTheDocument();
  });

  it("public's first tab is Permissions", async () => {
    serve({ "groups/public": { status: 200, body: PUBLIC } });
    open("/governance/people/groups/public?tab=nonsense");
    expect(await screen.findByRole("tab", { name: "Permissions", selected: true })).toBeInTheDocument();
  });

  it("moves between tabs with the arrow keys and keeps the tab in ?tab=", async () => {
    serve({ "users/u-ana": { status: 200, body: ANA } });
    open("/governance/people/users/u-ana");
    const overview = await screen.findByRole("tab", { name: "Overview", selected: true });
    await userEvent.click(overview);
    await userEvent.keyboard("{ArrowRight}");
    expect(await screen.findByRole("tab", { name: "Permissions", selected: true })).toHaveFocus();
    expect(where()).toBe("/governance/people/users/u-ana?tab=permissions");
    await userEvent.keyboard("{End}");
    expect(await screen.findByRole("tab", { name: "Check access", selected: true })).toBeInTheDocument();
    await userEvent.keyboard("{Home}");
    expect(await screen.findByRole("tab", { name: "Overview", selected: true })).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/u-ana");
  });
});

describe("principal page: header", () => {
  it("a user: name, email, username and status", async () => {
    serve({ "users/u-ana": { status: 200, body: ANA } });
    open("/governance/people/users/u-ana");
    const header = await screen.findByRole("banner");
    expect(await within(header).findByRole("heading", { level: 1, name: "Ana Silva" })).toBeInTheDocument();
    expect(within(header).getByText("ana@example.com")).toBeInTheDocument();
    expect(within(header).getByText("ana")).toBeInTheDocument();
    expect(within(header).getByText("Active")).toBeInTheDocument();
  });

  it("a user with no username says so and how one is allocated", async () => {
    serve({ "users/u-bo": { status: 200, body: { ...ANA, id: "u-bo", name: "Bo", username: null, status: "disabled" } } });
    open("/governance/people/users/u-bo");
    expect(
      await screen.findByText(
        "No username yet, so no engine knows this user. egret allocates one at their first query, notebook or engine token, or when a grant naming one of their groups or workspaces is compiled.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Disabled")).toBeInTheDocument();
  });

  it("marks a service account", async () => {
    serve({ "users/u-bot": { status: 200, body: { ...ANA, id: "u-bot", name: "etl-bot", serviceAccount: true } } });
    open("/governance/people/users/u-bot");
    expect(await within(await screen.findByRole("banner")).findByText("Service account")).toBeInTheDocument();
  });

  it("a group: source and member count; OIDC says its memberships change at login", async () => {
    serve({ "groups/eu/analysts": { status: 200, body: ANALYSTS } });
    open("/governance/people/groups/eu%2Fanalysts");
    const header = await screen.findByRole("banner");
    expect(await within(header).findByText("OIDC: memberships change at login")).toBeInTheDocument();
    expect(within(header).getByText("3 members (1 service account)")).toBeInTheDocument();
    expect(within(header).getByRole("heading", { level: 1, name: "eu/analysts" })).toBeInTheDocument();
  });

  it("public says what it is", async () => {
    serve({ "groups/public": { status: 200, body: PUBLIC } });
    open("/governance/people/groups/public");
    expect(
      await screen.findByText("Everyone. Engines resolve public themselves; it has no members to list."),
    ).toBeInTheDocument();
  });

  it("a role: the scopes it exists in and its holders", async () => {
    serve({ "roles/steward": { status: 200, body: STEWARD_ROLE } });
    open("/governance/people/roles/steward");
    const header = await screen.findByRole("banner");
    expect(await within(header).findByText("Exists in platform, workspace")).toBeInTheDocument();
    expect(within(header).getByText("2 holders")).toBeInTheDocument();
  });

  it.each<[string, string, Record<string, Answer>, string, string]>([
    ["user", "/governance/people/users/u-ana", { "users/u-ana": { status: 200, body: ANA } }, "user:read", "/control-plane/users/u-ana"],
    ["group", "/governance/people/groups/eu%2Fanalysts", { "groups/eu/analysts": { status: 200, body: ANALYSTS } }, "group:read", "/control-plane/groups"],
    ["role", "/governance/people/roles/steward", { "roles/steward": { status: 200, body: STEWARD_ROLE } }, "role:read", "/control-plane/roles"],
  ])("a %s offers Manage in Control Plane only with %s", async (_k, url, answers, perm, href) => {
    serve(answers);
    const view = open(url);
    await screen.findByRole("tab", { name: "Overview" });
    expect(screen.queryByRole("link", { name: "Manage in Control Plane" })).not.toBeInTheDocument();
    view.unmount();
    serve(answers);
    open(url, me(["policy:read", perm]));
    expect(await screen.findByRole("link", { name: "Manage in Control Plane" })).toHaveAttribute("href", href);
  });

  it("another kind's read permission does not offer it", async () => {
    serve({ "groups/eu/analysts": { status: 200, body: ANALYSTS } });
    open("/governance/people/groups/eu%2Fanalysts", me(["policy:read", "user:read", "role:read"]));
    await screen.findByRole("tab", { name: "Overview" });
    expect(screen.queryByRole("link", { name: "Manage in Control Plane" })).not.toBeInTheDocument();
  });
});

describe("principal page: Overview", () => {
  it("a user's memberships: scope, expiry and whether engines apply each, in words", async () => {
    serve({ "users/u-ana": { status: 200, body: ANA } });
    open("/governance/people/users/u-ana");
    const groups = await screen.findByRole("region", { name: "Groups" });
    const analysts = within(groups).getByRole("link", { name: "analysts" });
    expect(analysts).toHaveAttribute("href", "/governance/people/groups/analysts");
    expect(within(analysts.closest("tr")!).getByText("Yes")).toBeInTheDocument();

    const roles = screen.getByRole("region", { name: "Roles" });
    const steward = within(roles).getByRole("link", { name: "steward" }).closest("tr")!;
    expect(within(steward).getByText("Platform")).toBeInTheDocument();
    expect(within(steward).getByText("Direct")).toBeInTheDocument();
    expect(within(steward).getByText("2026-12-31")).toBeInTheDocument();
    expect(within(steward).getByText("Yes")).toBeInTheDocument();
    const auditor = within(roles).getByRole("link", { name: "auditor" }).closest("tr")!;
    expect(within(auditor).getByText("Expired 2026-09-01")).toBeInTheDocument();
    expect(
      within(auditor).getByText("No: expired. Engines may apply it until the next recompile."),
    ).toBeInTheDocument();
    const reader = within(roles).getByRole("link", { name: "reader" }).closest("tr")!;
    expect(within(reader).getByText("Workspace Sales")).toBeInTheDocument();
    expect(within(reader).getByRole("link", { name: "Group analysts" })).toHaveAttribute(
      "href",
      "/governance/people/groups/analysts",
    );
    expect(within(reader).getByText("Never")).toBeInTheDocument();
    expect(within(reader).getByText("egret only — not applied on engines: held through a group")).toBeInTheDocument();

    const ws = screen.getByRole("region", { name: "Workspaces" });
    const sales = within(ws).getByText("Sales").closest("tr")!;
    expect(within(sales).getByText("reader")).toBeInTheDocument();
    expect(within(sales).getByText("egret only — not applied on engines: held through a group")).toBeInTheDocument();
    for (const region of [groups, roles, ws]) expectStackedLabels(within(region).getByRole("table"));
  });

  it("a user with no username: memberships say so", async () => {
    serve({
      "users/u-bo": {
        status: 200,
        body: {
          ...ANA,
          id: "u-bo",
          username: null,
          groups: [membership({ via: { kind: "group", name: "analysts" }, onEngines: false, notOnEngines: "no_username" })],
          roles: [],
          workspaces: [],
        },
      },
    });
    open("/governance/people/users/u-bo");
    const groups = await screen.findByRole("region", { name: "Groups" });
    expect(within(groups).getByText("No: no username yet")).toBeInTheDocument();
    expect(screen.getByText("No roles.")).toBeInTheDocument();
    expect(screen.getByText("No workspaces.")).toBeInTheDocument();
  });

  it("a group's members, with the sentence for one with no username, and its roles and workspaces marked egret only", async () => {
    serve({ "groups/eu/analysts": { status: 200, body: ANALYSTS } });
    open("/governance/people/groups/eu%2Fanalysts");
    const members = await screen.findByRole("region", { name: "Members" });
    expect(within(members).getByRole("link", { name: "Ana Silva" })).toHaveAttribute("href", "/governance/people/users/u-ana");
    const bo = within(members).getByRole("link", { name: "Bo Chen" }).closest("tr")!;
    expect(
      within(bo).getByText("No username yet: a grant naming this group allocates one at compile"),
    ).toBeInTheDocument();
    const bot = within(members).getByRole("link", { name: "etl-bot" }).closest("tr")!;
    expect(within(bot).getByText("Service account")).toBeInTheDocument();
    const roles = screen.getByRole("region", { name: "Roles held" });
    expect(within(roles).getByText("egret only — not applied on engines: held through a group")).toBeInTheDocument();
    const ws = screen.getByRole("region", { name: "Workspaces held" });
    expect(within(ws).getByText("egret only — not applied on engines: held through a group")).toBeInTheDocument();
  });

  it("a role's holders, with the sentence for one with no username, and the groups holding it marked egret only", async () => {
    serve({ "roles/steward": { status: 200, body: STEWARD_ROLE } });
    open("/governance/people/roles/steward");
    const holders = await screen.findByRole("region", { name: "Holders" });
    const bo = within(holders).getByRole("link", { name: "Bo Chen" }).closest("tr")!;
    expect(within(bo).getByText("No username yet: a grant naming this role allocates one at compile")).toBeInTheDocument();
    const groups = screen.getByRole("region", { name: "Groups holding this role" });
    const analysts = within(groups).getByRole("link", { name: "analysts" }).closest("tr")!;
    expect(within(analysts).getByText("egret only — not applied on engines")).toBeInTheDocument();
    for (const region of [holders, groups]) expectStackedLabels(within(region).getByRole("table"));
  });

  it("an empty group says it has no members", async () => {
    serve({ "groups/solo": { status: 200, body: { ...ANALYSTS, name: "solo", source: "local", members: [], serviceAccounts: 0, roles: [], workspaces: [] } } });
    open("/governance/people/groups/solo");
    expect(await screen.findByText("No members. Grants naming this group match nobody.")).toBeInTheDocument();
    expect(screen.getByText("0 members")).toBeInTheDocument();
  });
});

describe("principal page: errors", () => {
  it.each<[string, string, string]>([
    ["group", "/governance/people/groups/gone", "No group named gone. Grants naming it match nobody; see Unmatched names."],
    ["role", "/governance/people/roles/gone", "No role named gone. Grants naming it match nobody; see Unmatched names."],
  ])("an unknown %s says grants naming it match nobody, and links Unmatched names", async (_k, url, text) => {
    serve({});
    open(url);
    const alert = await screen.findByText((_c, el) => el?.tagName === "P" && el.textContent === text);
    expect(within(alert).getByRole("link", { name: "Unmatched names" })).toHaveAttribute(
      "href",
      "/governance/people?kind=unmatched",
    );
    expect(screen.getByRole("heading", { level: 1, name: "gone" })).toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("an unknown user id says so", async () => {
    serve({});
    open("/governance/people/users/u-x");
    expect(await screen.findByText("No user has the id u-x.")).toBeInTheDocument();
  });

  it("another error keeps the header and offers a retry", async () => {
    let fail = true;
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(fail ? json(500, { error: "boom" }) : json(200, STEWARD_ROLE))));
    open("/governance/people/roles/steward", me(["policy:read", "role:read"]));
    expect(await screen.findByText("The role could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "steward" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Manage in Control Plane" })).toBeInTheDocument();
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("region", { name: "Holders" })).toBeInTheDocument();
  });

  it("loading shows the header from the URL and a skeleton", async () => {
    serve({ "groups/eu/analysts": "never" });
    open("/governance/people/groups/eu%2Fanalysts");
    expect(await screen.findByRole("heading", { level: 1, name: "eu/analysts" })).toBeInTheDocument();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });
});
