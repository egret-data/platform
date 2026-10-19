import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { EngineGrantView, GrantView, GroupPrincipal, RolePrincipal, StandsFor, UserPrincipal } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";
import { expectStackedLabels } from "@/routes/governance/people/__tests__/stackedTable";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

type Answer = { status: number; body: unknown } | "never";
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/**
 * Stubs `fetch` for a principal read (`GET …/perm/people/{kind}/{key}`), the
 * grants list (`GET …/perm/grants`, answered in order, the last repeating) and
 * a grant delete. Everything else is a 404. Returns the perm calls made.
 */
function serve(
  principal: Record<string, Answer>,
  grants: Answer | Answer[],
  engineGrants: Answer | Answer[] = { status: 200, body: [] },
  capabilities: Answer = { status: 200, body: { engines: [] } },
) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const method = (init?.method ?? "GET").toUpperCase();
      const reply = (a: Answer) => (a === "never" ? new Promise<Response>(() => {}) : Promise.resolve(json(a.status, a.body)));
      const m = /^\/api\/control-plane\/perm\/people\/(users|groups|roles)\/([^/]+)$/.exec(url.pathname);
      if (m) return reply(principal[`${m[1]}/${decodeURIComponent(m[2])}`] ?? { status: 404, body: { error: "not found" } });
      const perm = /^\/api\/control-plane(\/perm\/.+)$/.exec(url.pathname);
      if (perm) {
        const call = `${method} ${perm[1]}`;
        calls.push(call);
        if (call === "GET /perm/grants") {
          const asked = calls.filter((c) => c === call).length - 1;
          return reply(Array.isArray(grants) ? grants[Math.min(asked, grants.length - 1)] : grants);
        }
        if (call === "GET /perm/engine-grants") {
          const asked = calls.filter((c) => c === call).length - 1;
          return reply(Array.isArray(engineGrants) ? engineGrants[Math.min(asked, engineGrants.length - 1)] : engineGrants);
        }
        if (call === "GET /perm/engine-capabilities") return reply(capabilities);
        if (method === "DELETE") return reply({ status: 200, body: {} });
      }
      // #4b route B: pending unless a test answers it (`HW`), so egret's own sections are what these tests read.
      const hw = /^\/api\/control-plane\/governance\/principals\/(user|group|role)\/([^/]+)\/handwritten$/.exec(url.pathname);
      if (hw) {
        hwAsked.push(`${hw[1]}/${decodeURIComponent(hw[2])}`);
        const a = HW[`${hw[1]}/${decodeURIComponent(hw[2])}`];
        return a ? Promise.resolve(json(a.status, a.body)) : new Promise<Response>(() => {});
      }
      return Promise.resolve(json(404, { error: "not found" }));
    }),
  );
  return calls;
}

const me = (permissions: string[]): Me => ({ user_id: "s1", email: "s@example.com", platform_roles: [], permissions });
const READER = me(["policy:read"]);
const EDITOR = me(["policy:read", "policy:update"]);

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}
const where = () => screen.getByTestId("where").textContent;

function open(url: string, as: Me = READER) {
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
/** Route B's answers by `kind/id` (#4b); an unanswered one stays pending. Reset after each test. */
let HW: Record<string, { status: number; body: unknown }> = {};
const hwAsked: string[] = [];
afterEach(() => {
  HW = {};
  hwAsked.length = 0;
});
/** egret's own sections: the hand-written one (#4b) is tested on its own. */
const notHandWritten = (t: string | null) => !t?.startsWith("Hand-written") && !["Direct", "Through a group or role", "Applies to everyone"].includes(t ?? "");


const WS = "6f1c2a90-0000-4000-8000-00000000000a";

const STANDS_FOR: StandsFor[] = [
  { kind: "user", ref: "ana", via: { kind: "itself" }, onEngines: true },
  { kind: "group", ref: "analysts", via: { kind: "group", name: "analysts" }, onEngines: true },
  { kind: "role", ref: "steward", via: { kind: "role", name: "steward" }, onEngines: true },
  { kind: "role", ref: "reader", via: { kind: "group_role", group: "analysts", role: "reader" }, onEngines: false, notOnEngines: "held_through_group" },
  { kind: "workspace_members", ref: WS, via: { kind: "workspace", id: WS, name: "Sales" }, onEngines: true },
  { kind: "group", ref: "public", via: { kind: "public" }, onEngines: true },
];

const ANA: UserPrincipal = {
  id: "u-ana",
  email: "ana@example.com",
  name: "Ana Silva",
  username: "ana",
  status: "active",
  serviceAccount: false,
  groups: [],
  roles: [],
  workspaces: [],
  standsFor: STANDS_FOR,
  clearances: [],
};

const grant = (id: string, over: Partial<GrantView> & Pick<GrantView, "principals">): GrantView => ({
  id,
  effect: "allow",
  actions: ["read"],
  scope: { catalog_id: "c-ice", schema: "sales" },
  origin: { type: "explicit" },
  catalog_name: "ice",
  ...over,
});
const inc = (kind: "user" | "group" | "role" | "workspace_members", ref: string) => ({ include: [{ kind, ref }], exclude: [] });

// Not in the order the page lists them, on purpose.
const GRANTS: GrantView[] = [
  grant("g-egret", { principals: inc("role", "reader") }),
  grant("g-public", { principals: inc("group", "public"), scope: { catalog_id: "c-ice" } }),
  grant("g-excl", { principals: { include: [{ kind: "group", ref: "analysts" }], exclude: [{ kind: "user", ref: "ana" }] }, scope: { catalog_id: "c-ice", schema: "hr" } }),
  grant("g-ws", { principals: inc("workspace_members", WS.toUpperCase()), origin: { type: "workspace_assignment", workspace_id: WS } }),
  grant("g-role", { principals: inc("role", "steward"), actions: ["write"] }),
  grant("g-group", { principals: inc("group", "analysts"), effect: "deny", scope: { catalog_id: "c-ice", schema: "sales", table: "salaries" } }),
  grant("g-direct", { principals: inc("user", "ana"), actions: ["read", "write"] }),
  grant("g-other", { principals: inc("user", "bob") }),
  grant("g-case", { principals: inc("user", "Ana") }),
];

const ok = (body: unknown): Answer => ({ status: 200, body });

const tab = () => screen.getByRole("tabpanel");
const rowsOf = (name: string) =>
  within(within(tab()).getByRole("table", { name }))
    .getAllByRole("row")
    .slice(1)
    .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));

describe("a principal's Permissions tab", () => {
  it("groups the grants that reach the principal by how they reach it", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Directly" });
    expect(within(tab()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent).filter(notHandWritten)).toEqual([
      "Directly",
      "Through a group",
      "Through a role",
      "Through a workspace",
      "Everyone",
      "Excluded",
      "Not applied anywhere",
      "Engine permissions",
    ]);
    expect(rowsOf("Directly")).toEqual([["user ana can read and write ice.sales.*", "Directly · applied on engines", "Allow"]]);
    expect(rowsOf("Through a group")).toEqual([
      ["group analysts cannot read ice.sales.salaries", "Through group analysts · applied on engines", "Deny"],
    ]);
    expect(rowsOf("Through a role")).toEqual([["role steward can write ice.sales.*", "Through role steward · applied on engines", "Allow"]]);
    expect(rowsOf("Through a workspace")).toEqual([
      [
        `members of workspace Sales can read ice.sales.*Set by workspace assignment · Sales`,
        "Through workspace Sales · applied on engines",
        "Allow",
      ],
    ]);
    expect(rowsOf("Everyone")).toEqual([["group public can read ice.*", "Everyone (public) · applied on engines", "Allow"]]);
    expect(rowsOf("Excluded")).toEqual([
      [
        "group analysts can read ice.hr.* except user ana",
        "Through group analysts · applied on enginesExcluded: directly · applied on engines",
        "Allow",
      ],
    ]);
    expect(rowsOf("Not applied anywhere")).toEqual([
      [
        "role reader can read ice.sales.*",
        "Through role reader, held by group analysts · not applied anywhere: held through a group",
        "Allow",
      ],
    ]);
    // Final review I1: egret's own checks never read grants, so a group-held role reaches nowhere.
    expect(
      within(within(tab()).getByRole("region", { name: "Not applied anywhere" })).getByText(
        "These reach this principal only through a role or workspace a group holds, which engines do not apply. egret's own checks do not read grants either, so a grant that reaches only this way applies to this principal nowhere.",
      ),
    ).toBeInTheDocument();
    // A grant naming someone else, or the username in another case, is not listed.
    expect(tab().textContent).not.toMatch(/bob|user Ana /);
  });

  it("a deny excepting a role held only through a group is listed by its include: engines still deny", async () => {
    const DENY = grant("g-deny", {
      effect: "deny",
      principals: { include: [{ kind: "group", ref: "analysts" }], exclude: [{ kind: "role", ref: "reader" }] },
    });
    // An exclude engines do not apply, with no include, concerns nobody: not listed.
    const NOTHING = grant("g-nothing", { principals: { include: [{ kind: "user", ref: "bob" }], exclude: [{ kind: "role", ref: "reader" }] } });
    serve({ "users/u-ana": ok(ANA) }, ok([DENY, NOTHING]));
    open("/governance/people/users/u-ana?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Through a group" });
    expect(within(tab()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent).filter(notHandWritten)).toEqual(["Through a group", "Engine permissions"]);
    expect(rowsOf("Through a group")).toEqual([
      [
        "group analysts cannot read ice.sales.* except role reader",
        "Through group analysts · applied on enginesExcludes nobody: through role reader, held by group analysts — engines do not apply it, and egret's own checks do not read grants",
        "Deny",
      ],
    ]);
  });

  it("a user with no username: grants that would reach them are not applied on engines, and the page says why", async () => {
    const NO_NAME: UserPrincipal = {
      ...ANA,
      username: null,
      standsFor: [
        { kind: "group", ref: "analysts", via: { kind: "group", name: "analysts" }, onEngines: false, notOnEngines: "no_username" },
        { kind: "role", ref: "reader", via: { kind: "group_role", group: "analysts", role: "reader" }, onEngines: false, notOnEngines: "held_through_group" },
      ],
    };
    serve({ "users/u-ana": ok(NO_NAME) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    const panel = await screen.findByRole("tabpanel");
    expect(
      await within(panel).findByText("No engine knows this user until they have a username; nothing below applies on engines."),
    ).toBeInTheDocument();
    expect(within(panel).getAllByRole("heading", { level: 2 }).map((h) => h.textContent).filter(notHandWritten)).toEqual([
      "Not applied anywhere",
      "Not applied on engines",
      "Engine permissions",
    ]);
    expect(rowsOf("Not applied on engines")).toEqual([
      ["group analysts can read ice.hr.* except user ana", "Through group analysts · No: no username yet", "Allow"],
      ["group analysts cannot read ice.sales.salaries", "Through group analysts · No: no username yet", "Deny"],
    ]);
  });

  it("public's note names no Check access, which public does not have", async () => {
    const PUBLIC: GroupPrincipal = {
      name: "public",
      source: null,
      isPublic: true,
      members: [],
      serviceAccounts: 0,
      roles: [],
      workspaces: [],
      standsFor: [{ kind: "group", ref: "public", via: { kind: "public" }, onEngines: true }],
      clearances: [],
    };
    serve({ "groups/public": ok(PUBLIC) }, ok(GRANTS));
    open("/governance/people/groups/public");
    const note = await within(await screen.findByRole("tabpanel")).findByText(/^egret grants are above;/);
    expect(note.textContent).toBe("egret grants are above; hand-written policies below.");
    expect(within(note).queryByRole("link")).not.toBeInTheDocument();
  });

  it("deleting a pattern grant says how far it reaches", async () => {
    const PATTERN = grant("g-pat", { principals: inc("user", "ana"), scope: { catalog_id: "c-ice", schema: "sales", table: "ord*" } });
    serve({ "users/u-ana": ok(ANA) }, ok([PATTERN]));
    open("/governance/people/users/u-ana?tab=permissions", EDITOR);
    await userEvent.click(
      await within(await screen.findByRole("tabpanel")).findByRole("button", { name: "Delete user ana can read ice.sales.“ord*”" }),
    );
    expect(
      within(await screen.findByRole("alertdialog")).getByText(
        "This grant covers every table matching “ord*”, not one object. It is permanently removed and every profile it reaches recompiles.",
      ),
    ).toBeInTheDocument();
  });

  it("points at the hand-written policies below, with a link to Check access (P10)", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    const note = await within(await screen.findByRole("tabpanel")).findByText(/^egret grants are above;/);
    expect(note).toHaveTextContent(
      "egret grants are above; hand-written policies below. To see the engines' answer, use Check access.",
    );
    await userEvent.click(within(note).getByRole("link", { name: "Check access" }));
    expect(where()).toBe("/governance/people/users/u-ana?tab=check");
  });

  it("links each principal of a grant to its People page", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    const group = await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Through a group" });
    expect(within(group).getByRole("link", { name: "analysts" })).toHaveAttribute("href", "/governance/people/groups/analysts");
  });

  it("filters by effect and action through ?effect= and ?action=", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions&action=write");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Directly" });
    expect(within(tab()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent).filter(notHandWritten)).toEqual(["Directly", "Through a role", "Engine permissions"]);

    await userEvent.click(within(tab()).getByRole("combobox", { name: "Effect" }));
    await userEvent.click(await screen.findByRole("option", { name: "Deny" }));
    expect(await within(tab()).findByText("No grant matches these filters.")).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/u-ana?tab=permissions&action=write&effect=deny");
    await userEvent.click(within(tab()).getByRole("button", { name: "Clear filters" }));
    expect(await within(tab()).findByRole("table", { name: "Not applied anywhere" })).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/u-ana?tab=permissions");
  });

  it("says when no egret grant reaches the principal", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok([GRANTS[7]]));
    open("/governance/people/users/u-ana?tab=permissions");
    expect(await within(await screen.findByRole("tabpanel")).findByText("No egret grant reaches Ana Silva")).toBeInTheDocument();
  });

  it("shows the grants' error with a retry, never an empty list", async () => {
    serve({ "users/u-ana": ok(ANA) }, [{ status: 500, body: { error: "boom" } }, ok(GRANTS)]);
    open("/governance/people/users/u-ana?tab=permissions");
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("The grants could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel).queryByText(/No egret grant reaches/)).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: "Retry" }));
    expect(await within(panel).findByRole("table", { name: "Directly" })).toBeInTheDocument();
  });

  it("offers Edit and Delete with policy:update, on explicit grants only", async () => {
    const calls = serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions", EDITOR);
    const ws = await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Through a workspace" });
    expect(within(ws).queryByRole("button", { name: /^Edit|^Delete/ })).not.toBeInTheDocument();
    // Live check D6: below sm each row is a card, with Effect labelled and the actions in its flow.
    for (const table of within(tab()).getAllByRole("table")) expectStackedLabels(table);

    await userEvent.click(within(tab()).getByRole("button", { name: "Edit user ana can read and write ice.sales.*" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Edit grant" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    await userEvent.click(within(tab()).getByRole("button", { name: "Delete role steward can write ice.sales.*" }));
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.type(within(confirm).getByRole("textbox"), "delete");
    await userEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toContain("DELETE /perm/grants/g-role"));
    await waitFor(() => expect(calls.filter((c) => c === "GET /perm/grants")).toHaveLength(2));
  });

  it("offers neither without policy:update", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Directly" });
    expect(within(tab()).queryByRole("button", { name: /^Edit|^Delete/ })).not.toBeInTheDocument();
    for (const table of within(tab()).getAllByRole("table")) expectStackedLabels(table);
  });
});

describe("Grant access", () => {
  const ANALYSTS: GroupPrincipal = {
    name: "eu/analysts",
    source: "local",
    isPublic: false,
    members: [],
    serviceAccounts: 0,
    roles: [],
    workspaces: [],
    standsFor: [{ kind: "group", ref: "eu/analysts", via: { kind: "group", name: "eu/analysts" }, onEngines: true }],
    clearances: [],
  };

  it("opens a new grant with this principal in include: a user by username", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok([]));
    open("/governance/people/users/u-ana", EDITOR);
    await userEvent.click(await screen.findByRole("button", { name: "Grant access" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "New grant" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /^Remove ana from/ })).toBeInTheDocument();
  });

  it("a group by name", async () => {
    serve({ "groups/eu/analysts": ok(ANALYSTS) }, ok([]));
    open("/governance/people/groups/eu%2Fanalysts", EDITOR);
    await userEvent.click(await screen.findByRole("button", { name: "Grant access" }));
    expect(within(await screen.findByRole("dialog")).getByRole("button", { name: /^Remove eu\/analysts from/ })).toBeInTheDocument();
  });

  it("is disabled, with the reason, for a user with no username", async () => {
    serve({ "users/u-bo": ok({ ...ANA, id: "u-bo", username: null, standsFor: [] }) }, ok([]));
    open("/governance/people/users/u-bo", EDITOR);
    const button = await screen.findByRole("button", { name: "Grant access" });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription("A grant names a user by username, and this user has none yet.");
  });

  it("is not offered without policy:update", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok([]));
    open("/governance/people/users/u-ana");
    await screen.findByRole("tab", { name: "Overview" });
    expect(screen.queryByRole("button", { name: "Grant access" })).not.toBeInTheDocument();
  });
});

describe("Engine permissions on a principal page (spec §7)", () => {
  const eg = (id: string, over: Partial<EngineGrantView> & Pick<EngineGrantView, "principals">): EngineGrantView => ({
    id,
    engineId: "e-trino",
    engineName: "trino-main",
    capability: "trino.impersonate_user",
    values: ["bob"],
    targetUserId: "u-bob",
    policyName: `perm-engine-${id}`,
    reach: { engines: [], sharedProfile: false },
    ...over,
  });
  const CAPS = {
    engines: [
      {
        engineType: "trino",
        capabilities: [
          { id: "trino.impersonate_user", label: "Impersonate a user", description: "", resources: [], access: "impersonate" },
          { id: "trino.set_catalog_session_property", label: "Set catalog session properties", description: "", resources: [], access: "alter" },
        ],
        notGrantable: [],
      },
      {
        engineType: "starrocks",
        capabilities: [{ id: "starrocks.impersonate_user", label: "Impersonate a user (StarRocks)", description: "", resources: [], access: "impersonate" }],
        notGrantable: [],
      },
    ],
  };
  // Not in the order the section lists them, on purpose.
  const ENGINE_GRANTS: EngineGrantView[] = [
    eg("eg-other", { principals: [{ kind: "user", ref: "bob" }], values: ["ana"] }),
    eg("eg-group", {
      principals: [{ kind: "group", ref: "analysts" }],
      capability: "trino.set_catalog_session_property",
      values: ["ice", "query_max_memory"],
      targetUserId: null,
    }),
    eg("eg-direct", {
      principals: [{ kind: "user", ref: "ana" }],
      engineId: "e-sr",
      engineName: "sr-main",
      capability: "starrocks.impersonate_user",
      values: [],
      targetUserId: "u-nouser",
    }),
    eg("eg-role", { principals: [{ kind: "role", ref: "steward" }], engineId: "e-sr", engineName: "sr-main", capability: "starrocks.impersonate_user" }),
  ];
  const section = () => within(tab()).getByRole("region", { name: "Engine permissions" });
  const engineRows = () =>
    within(within(section()).getByRole("table", { name: "Engine permissions" }))
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));

  it("a user page lists the engine grants that reach the user, directly or through a group, and not another user's", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok([]), ok(ENGINE_GRANTS), ok(CAPS));
    open("/governance/people/users/u-ana?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Engine permissions" });
    expect(engineRows()).toEqual([
      ["sr-main", "Impersonate a user (StarRocks)", "a user with no username", "Directly · applied on engines"],
      ["sr-main", "Impersonate a user (StarRocks)", "bob", "Through role steward · applied on engines"],
      ["trino-main", "Set catalog session properties", "ice, query_max_memory", "Through group analysts · applied on engines"],
    ]);
    expect(section().textContent).not.toMatch(/eg-other/);
    expect(within(section()).getAllByRole("link", { name: "sr-main" })[0]).toHaveAttribute(
      "href",
      "/governance/engines/e-sr?tab=engine-permissions&grant=eg-direct",
    );
    expect(within(section()).getByRole("link", { name: "trino-main" })).toHaveAttribute(
      "href",
      "/governance/engines/e-trino?tab=engine-permissions&grant=eg-group",
    );
  });

  it("a group page lists the grants naming the group", async () => {
    const ANALYSTS: GroupPrincipal = {
      name: "analysts",
      source: null,
      isPublic: false,
      members: [],
      serviceAccounts: 0,
      roles: [],
      workspaces: [],
      standsFor: [{ kind: "group", ref: "analysts", via: { kind: "itself" }, onEngines: true }],
      clearances: [],
    };
    serve({ "groups/analysts": ok(ANALYSTS) }, ok([]), ok(ENGINE_GRANTS), ok(CAPS));
    open("/governance/people/groups/analysts?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Engine permissions" });
    expect(engineRows()).toEqual([["trino-main", "Set catalog session properties", "ice, query_max_memory", "Directly · applied on engines"]]);
  });

  it("a role page lists the grants naming the role", async () => {
    const STEWARD: RolePrincipal = {
      name: "steward",
      scopes: [],
      holders: [],
      serviceAccounts: 0,
      holderGroups: [],
      standsFor: [{ kind: "role", ref: "steward", via: { kind: "itself" }, onEngines: true }],
      clearances: [],
    };
    serve({ "roles/steward": ok(STEWARD) }, ok([]), ok(ENGINE_GRANTS), ok(CAPS));
    open("/governance/people/roles/steward?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Engine permissions" });
    expect(engineRows()).toEqual([["sr-main", "Impersonate a user (StarRocks)", "bob", "Directly · applied on engines"]]);
  });

  it("the public group's page has no engine permissions section", async () => {
    const PUBLIC: GroupPrincipal = {
      name: "public",
      source: null,
      isPublic: true,
      members: [],
      serviceAccounts: 0,
      roles: [],
      workspaces: [],
      standsFor: [{ kind: "group", ref: "public", via: { kind: "public" }, onEngines: true }],
      clearances: [],
    };
    const calls = serve({ "groups/public": ok(PUBLIC) }, ok([]), ok(ENGINE_GRANTS), ok(CAPS));
    open("/governance/people/groups/public?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByText(/No egret grant reaches/);
    expect(within(tab()).queryByRole("region", { name: "Engine permissions" })).not.toBeInTheDocument();
    expect(calls).not.toContain("GET /perm/engine-grants");
  });

  it("says when no engine permission reaches the principal", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok([]), ok([ENGINE_GRANTS[0]]), ok(CAPS));
    open("/governance/people/users/u-ana?tab=permissions");
    expect(
      await within(await screen.findByRole("tabpanel")).findByText("No engine permission reaches this user."),
    ).toBeInTheDocument();
  });

  it("a failing engine-grants request leaves the data grants rendered, with its own retry", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS), [{ status: 500, body: { error: "boom" } }, ok(ENGINE_GRANTS)], ok(CAPS));
    open("/governance/people/users/u-ana?tab=permissions");
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("The engine permissions could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel).getByRole("table", { name: "Directly" })).toBeInTheDocument();
    await userEvent.click(within(section()).getByRole("button", { name: "Retry" }));
    expect(await within(panel).findByRole("table", { name: "Engine permissions" })).toBeInTheDocument();
  });

  it("a failing data-grants request leaves the engine permissions rendered", async () => {
    serve({ "users/u-ana": ok(ANA) }, { status: 500, body: { error: "boom" } }, ok(ENGINE_GRANTS), ok(CAPS));
    open("/governance/people/users/u-ana?tab=permissions");
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("The grants could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(await within(panel).findByRole("table", { name: "Engine permissions" })).toBeInTheDocument();
  });
});

/** #4b spec §6: the hand-written access policies naming the principal (route B). */
describe("hand-written policies on a People page's Permissions tab", () => {
  const listed = (over: Record<string, unknown> = {}) => ({
    principal: { kind: "user", id: "u-ana", label: "Ana", username: "ana" },
    state: "listed",
    policies: [
      {
        id: 1,
        name: "hw-direct",
        service: "lake-tr",
        inTagService: false,
        policyType: "access",
        resourceSummary: "catalog=ice",
        enabled: true,
        denyAllElse: false,
        unmodelled: [],
        naming: [{ bucket: "allow", how: "direct", via: null, onEngines: true }],
        engines: [{ id: "e-tr", name: "lake-trino" }],
      },
      {
        id: 2,
        name: "hw-group",
        service: "lake-tr",
        inTagService: false,
        policyType: "access",
        resourceSummary: "catalog=ice",
        enabled: true,
        denyAllElse: false,
        unmodelled: [],
        naming: [{ bucket: "allow", how: "group", via: "analysts", onEngines: false }],
        engines: [{ id: "e-tr", name: "lake-trino" }],
      },
    ],
    everyone: { count: 1, policies: [{ id: 3, name: "hw-all", service: "lake-tr", bucket: "allow" }] },
    unreadable: [],
    ...over,
  });
  const GROUP: GroupPrincipal = {
    name: "analysts",
    source: "local",
    isPublic: false,
    members: [],
    serviceAccounts: 0,
    roles: [],
    workspaces: [],
    standsFor: [],
    clearances: [],
  };
  const ROLE: RolePrincipal = { name: "steward", scopes: ["platform"], holders: [], serviceAccounts: 0, holderGroups: [], standsFor: [], clearances: [] };

  it("a user's page groups rows Direct, Through a group or role, Applies to everyone", async () => {
    HW = { "user/u-ana": ok(listed()) as { status: number; body: unknown } };
    serve({ "users/u-ana": ok(ANA) }, ok([]));
    open("/governance/people/users/u-ana?tab=permissions");
    const section = await within(await screen.findByRole("tabpanel")).findByRole("region", { name: "Hand-written policies (2)" });
    expect(within(within(section).getByRole("region", { name: "Direct" })).getByRole("link", { name: "hw-direct" })).toBeInTheDocument();
    const via = within(section).getByRole("region", { name: "Through a group or role" });
    expect(via).toHaveTextContent("Names group analysts, which this user is in. Engines resolve no groups, so on the engine it reaches nobody.");
    expect(within(section).getByRole("region", { name: "Applies to everyone" })).toHaveTextContent("1 hand-written policy names everyone.");
    expect(hwAsked).toEqual(["user/u-ana"]);
  });

  it("a group's page and a role's page ask route B by name", async () => {
    serve({ "groups/analysts": ok(GROUP) }, ok([]));
    const g = open("/governance/people/groups/analysts?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("region", { name: "Hand-written policies" });
    expect(hwAsked).toEqual(["group/analysts"]);
    g.unmount();
    hwAsked.length = 0;
    serve({ "roles/steward": ok(ROLE) }, ok([]));
    open("/governance/people/roles/steward?tab=permissions");
    await within(await screen.findByRole("tabpanel")).findByRole("region", { name: "Hand-written policies" });
    expect(hwAsked).toEqual(["role/steward"]);
  });

  it("the public page has no hand-written section and asks nothing", async () => {
    serve({ "groups/public": ok({ ...GROUP, name: "public", source: null, isPublic: true }) }, ok([]));
    open("/governance/people/groups/public");
    await within(await screen.findByRole("tabpanel")).findByText(/^egret grants are above;/);
    expect(screen.queryByRole("region", { name: /^Hand-written policies/ })).not.toBeInTheDocument();
    expect(hwAsked).toEqual([]);
  });

  it("a user without a username is told no item can name them", async () => {
    HW = {
      "user/u-ana": ok(listed({ state: "no_username", policies: [], principal: { kind: "user", id: "u-ana", label: "Ana", username: null } })) as {
        status: number;
        body: unknown;
      },
    };
    serve({ "users/u-ana": ok({ ...ANA, username: null }) }, ok([]));
    open("/governance/people/users/u-ana?tab=permissions");
    expect(
      await within(await screen.findByRole("tabpanel")).findByText(
        "This user has no username, so no hand-written policy item can name them on an engine.",
      ),
    ).toBeInTheDocument();
  });

  it("a failing read is an error, never none", async () => {
    HW = { "user/u-ana": { status: 500, body: { error: "boom" } } };
    serve({ "users/u-ana": ok(ANA) }, ok([]));
    open("/governance/people/users/u-ana?tab=permissions");
    expect(await within(await screen.findByRole("tabpanel")).findByText(/The hand-written policies could not be loaded/)).toBeInTheDocument();
    expect(screen.queryByText("No hand-written policy names this principal.")).not.toBeInTheDocument();
  });
});
