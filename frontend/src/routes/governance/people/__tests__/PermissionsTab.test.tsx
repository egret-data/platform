import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { GrantView, GroupPrincipal, StandsFor, UserPrincipal } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";

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
function serve(principal: Record<string, Answer>, grants: Answer | Answer[]) {
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
        if (method === "DELETE") return reply({ status: 200, body: {} });
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
    expect(within(tab()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual([
      "Directly",
      "Through a group",
      "Through a role",
      "Through a workspace",
      "Everyone",
      "Excluded",
      "egret only",
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
    expect(rowsOf("egret only")).toEqual([
      [
        "role reader can read ice.sales.*",
        "Through role reader, held by group analysts · egret only — not applied on engines: held through a group",
        "Allow",
      ],
    ]);
    // A grant naming someone else, or the username in another case, is not listed.
    expect(tab().textContent).not.toMatch(/bob|user Ana /);
  });

  it("says grants are egret's only, with a link to Check access (P10)", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(GRANTS));
    open("/governance/people/users/u-ana?tab=permissions");
    const note = await within(await screen.findByRole("tabpanel")).findByText(/Lists egret grants\./);
    expect(note).toHaveTextContent(
      "Lists egret grants. Hand-written and egret-seeded engine policies are not listed here; Check access gives the engines' answer.",
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
    expect(within(tab()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual(["Directly", "Through a role"]);

    await userEvent.click(within(tab()).getByRole("combobox", { name: "Effect" }));
    await userEvent.click(await screen.findByRole("option", { name: "Deny" }));
    expect(await within(tab()).findByText("No grant matches these filters.")).toBeInTheDocument();
    expect(where()).toBe("/governance/people/users/u-ana?tab=permissions&action=write&effect=deny");
    await userEvent.click(within(tab()).getByRole("button", { name: "Clear filters" }));
    expect(await within(tab()).findByRole("table", { name: "egret only" })).toBeInTheDocument();
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
