import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { GovernanceCatalog, GroupPrincipal, RuleView, StandsFor, UserPrincipal } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";

type Answer = { status: number; body: unknown };
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const ok = (body: unknown): Answer => ({ status: 200, body });

/**
 * Stubs `fetch` for a principal read, the rules list (answered in order, the
 * last repeating) and the Governance catalog list. Everything else is a 404.
 */
function serve(principal: Record<string, Answer>, rules: Answer | Answer[], catalogs: Answer = ok(CATALOGS)) {
  const asked = { rules: 0 };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const reply = (a: Answer) => Promise.resolve(json(a.status, a.body));
      const m = /^\/api\/control-plane\/perm\/people\/(users|groups|roles)\/([^/]+)$/.exec(url.pathname);
      if (m) return reply(principal[`${m[1]}/${decodeURIComponent(m[2])}`] ?? { status: 404, body: { error: "not found" } });
      if (url.pathname === "/api/control-plane/perm/rules") {
        const list = Array.isArray(rules) ? rules : [rules];
        return reply(list[Math.min(asked.rules++, list.length - 1)]);
      }
      if (url.pathname === "/api/control-plane/governance/catalogs") return reply(catalogs);
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
}

const me = (permissions: string[]): Me => ({ user_id: "s1", email: "s@example.com", platform_roles: [], permissions });
const READER = me(["policy:read"]);

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
const WS2 = "6f1c2a90-0000-4000-8000-00000000000b";

const STANDS_FOR: StandsFor[] = [
  { kind: "user", ref: "ana", via: { kind: "itself" }, onEngines: true },
  { kind: "group", ref: "analysts", via: { kind: "group", name: "analysts" }, onEngines: true },
  { kind: "workspace_members", ref: WS, via: { kind: "workspace", id: WS, name: "Sales" }, onEngines: true },
  {
    kind: "workspace_members",
    ref: WS2,
    via: { kind: "group_workspace", group: "analysts", id: WS2, name: "Finance" },
    onEngines: false,
    notOnEngines: "held_through_group",
  },
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

const CATALOGS: GovernanceCatalog[] = [
  { id: "c-ice", name: "ice", source: "egret", connectorType: "iceberg", engines: [], workspaces: [] },
];

const exempt = (kind: "user" | "group" | "workspace_members", ref: string) => ({ include: [{ kind, ref }], exclude: [] });
const rule = (id: string, name: string, over: Partial<RuleView> & Pick<RuleView, "exempt">): RuleView => ({
  id,
  name,
  kind: { type: "row_filter", filter_expr: "region = 'EU'" },
  selector: { type: "path", catalog_id: "c-ice", schema: "sales", table: "orders" },
  catalog_name: "ice",
  ...over,
});

// Not in the order the page lists them, on purpose; 9 rules, 5 name Ana in their exemption.
const RULES: RuleView[] = [
  // Ana only in an exclude engines do not apply: it changes nothing for her anywhere, so it is not listed.
  rule("r-excl-egret", "bob but not finance", {
    exempt: { include: [{ kind: "user", ref: "bob" }], exclude: [{ kind: "workspace_members", ref: WS2 }] },
  }),
  rule("r-excl-on", "analysts but not ana", {
    exempt: { include: [{ kind: "group", ref: "analysts" }], exclude: [{ kind: "user", ref: "ana" }] },
  }),
  rule("r-gws", "finance filter", { exempt: exempt("workspace_members", WS2) }),
  rule("r-other", "bob only", { exempt: exempt("user", "bob") }),
  rule("r-group", "pii mask", {
    exempt: exempt("group", "analysts"),
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key: "pii" },
    catalog_name: null,
    tag_name: "PII",
  }),
  rule("r-none", "everyone", { exempt: { include: [], exclude: [] } }),
  rule("r-ws", "sales filter", { exempt: exempt("workspace_members", WS.toUpperCase()) }),
  rule("r-case", "case differs", { exempt: exempt("user", "Ana") }),
  rule("r-direct", "salary mask", {
    exempt: exempt("user", "ana"),
    kind: { type: "mask", mask_type: "MASK_NULL" },
    selector: { type: "path", catalog_id: "c-ice", schema: "hr", table: "pay", column: "salary" },
  }),
];

const tab = () => screen.getByRole("tabpanel");
const sections = () =>
  within(tab())
    .getAllByRole("region")
    .map((r) => within(r).getAllByRole("heading")[0].textContent)
    .filter(notHandWritten);
const rowsOf = (name: string) =>
  within(within(tab()).getByRole("table", { name }))
    .getAllByRole("row")
    .slice(1);

describe("a principal's Rules tab", () => {
  it("lists the rules that exempt the principal, grouped by how the exemption reaches", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES));
    open("/governance/people/users/u-ana?tab=masks");

    expect(await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Directly" })).toBeInTheDocument();
    expect(sections()).toEqual(["Directly", "Through a group", "Through a workspace", "Excluded from the exemption", "Not applied anywhere"]);

    const [direct] = rowsOf("Directly");
    expect(within(direct).getByRole("link", { name: "salary mask" })).toHaveAttribute("href", "/governance/rules/r-direct");
    expect(direct).toHaveTextContent("Mask · Nullify");
    expect(direct).toHaveTextContent("Directly · applied on engines");

    const [group] = rowsOf("Through a group");
    expect(within(group).getByRole("link", { name: "pii mask" })).toHaveAttribute("href", "/governance/rules/r-group");
    expect(group).toHaveTextContent("Classification PII");
    expect(group).toHaveTextContent("Through group analysts · applied on engines");

    const [excluded] = rowsOf("Excluded from the exemption");
    expect(within(excluded).getByRole("link", { name: "analysts but not ana" })).toBeInTheDocument();
    expect(excluded).toHaveTextContent("Excluded: directly · applied on engines");

    const [ws] = rowsOf("Through a workspace");
    expect(ws).toHaveTextContent("Through workspace Sales · applied on engines");

    const [egretOnly] = rowsOf("Not applied anywhere");
    expect(egretOnly).toHaveTextContent("Through workspace Finance, held by group analysts · not applied anywhere: held through a group");
    // egret itself applies no mask or filter, so an exemption engines skip exempts nobody.
    expect(within(tab()).getByText(/egret applies no mask or row filter itself/)).toBeInTheDocument();

    expect(within(tab()).queryByRole("link", { name: "bob only" })).not.toBeInTheDocument();
    expect(within(tab()).queryByRole("link", { name: "case differs" })).not.toBeInTheDocument();
    expect(within(tab()).queryByRole("link", { name: "everyone" })).not.toBeInTheDocument();
    expect(within(tab()).queryByRole("link", { name: "bob but not finance" })).not.toBeInTheDocument();
  });

  it("states how many other rules apply, and that each object's tab lists them", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES));
    open("/governance/people/users/u-ana?tab=masks");

    expect(
      await within(await screen.findByRole("tabpanel")).findByText(
        "4 other rules apply to everyone they do not exempt; each object's Rules tab lists them.",
      ),
    ).toBeInTheDocument();
  });

  it("writes the one-rule line in the singular", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES.filter((r) => !["r-other", "r-none", "r-excl-egret"].includes(r.id))));
    open("/governance/people/users/u-ana?tab=masks");

    expect(
      await within(await screen.findByRole("tabpanel")).findByText(
        "1 other rule applies to everyone it does not exempt; each object's Rules tab lists it.",
      ),
    ).toBeInTheDocument();
  });

  it("links a path rule to its object's Rules tab in Data, and not a tag rule", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES));
    open("/governance/people/users/u-ana?tab=masks");

    const [direct] = await within(await screen.findByRole("tabpanel")).findAllByRole("row", { name: /salary mask/ });
    expect(await within(direct).findByRole("link", { name: /Open in Data: salary mask/ })).toHaveAttribute(
      "href",
      "/governance/data/ice/hr/pay?column=salary&tab=masks",
    );
    const [group] = rowsOf("Through a group");
    expect(within(group).queryByRole("link", { name: /Open in Data/ })).not.toBeInTheDocument();
  });

  it("says rules only, and points at Check access", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES));
    open("/governance/people/users/u-ana?tab=masks");

    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText(/^Rules are above; hand-written policies below\./)).toBeInTheDocument();
    expect(within(panel).getByRole("link", { name: "Check access" })).toHaveAttribute(
      "href",
      "/governance/people/users/u-ana?tab=check",
    );
    // The control the note names exists under that name (FE-COPY-3).
    expect(screen.getByRole("tab", { name: "Check access" })).toBeInTheDocument();
  });

  it("states the fact when no rule exempts the principal, and still counts the rest", async () => {
    serve({ "users/u-ana": ok(ANA) }, ok(RULES.filter((r) => r.id === "r-other" || r.id === "r-none")));
    open("/governance/people/users/u-ana?tab=masks");

    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("No rule exempts Ana Silva")).toBeInTheDocument();
    expect(
      within(panel).getByText("2 other rules apply to everyone they do not exempt; each object's Rules tab lists them."),
    ).toBeInTheDocument();
  });

  it("shows a failed rules read as an error with a retry, never as empty", async () => {
    serve({ "users/u-ana": ok(ANA) }, [{ status: 500, body: { error: "boom" } }, ok(RULES)]);
    open("/governance/people/users/u-ana?tab=masks");

    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel).queryByText(/No rule exempts/)).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: "Retry" }));
    expect(await within(panel).findByRole("table", { name: "Directly" })).toBeInTheDocument();
  });

  it("says nothing below applies on engines for a user with no username", async () => {
    const noName: UserPrincipal = {
      ...ANA,
      username: null,
      standsFor: [
        { kind: "group", ref: "analysts", via: { kind: "group", name: "analysts" }, onEngines: false, notOnEngines: "no_username" },
      ],
    };
    serve({ "users/u-ana": ok(noName) }, ok(RULES));
    open("/governance/people/users/u-ana?tab=masks");

    const panel = await screen.findByRole("tabpanel");
    expect(
      await within(panel).findByText("No engine knows this user until they have a username; nothing below applies on engines."),
    ).toBeInTheDocument();
    const [row] = rowsOf("Not applied on engines");
    expect(row).toHaveTextContent("Through group analysts · No: no username yet");
    // Said of a path held through a group only.
    expect(within(panel).queryByText(/egret applies no mask or row filter itself/)).not.toBeInTheDocument();
  });

  it("lists a group's exemptions on the group's page", async () => {
    const analysts: GroupPrincipal = {
      name: "analysts",
      source: "local",
      isPublic: false,
      members: [],
      serviceAccounts: 0,
      roles: [],
      workspaces: [],
      standsFor: [
        { kind: "group", ref: "analysts", via: { kind: "itself" }, onEngines: true },
        { kind: "group", ref: "public", via: { kind: "public" }, onEngines: true },
      ],
      clearances: [],
    };
    serve({ "groups/analysts": ok(analysts) }, ok(RULES));
    open("/governance/people/groups/analysts?tab=masks");

    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByRole("link", { name: "pii mask" })).toBeInTheDocument();
    expect(sections()).toEqual(["Directly"]);
    expect(within(panel).getByRole("link", { name: "analysts but not ana" })).toBeInTheDocument();
    expect(
      within(panel).getByText("7 other rules apply to everyone they do not exempt; each object's Rules tab lists them."),
    ).toBeInTheDocument();
  });
});

/** #4b spec §6: the hand-written masking and row-filter policies naming the principal (route B). */
describe("hand-written policies on a People page's Rules tab", () => {
  it("lists the masking and row-filter rows, not the access ones", async () => {
    const row = (id: number, name: string, policyType: string, bucket: string) => ({
      id,
      name,
      service: "lake-tr",
      inTagService: false,
      policyType,
      resourceSummary: "catalog=ice",
      enabled: true,
      denyAllElse: false,
      unmodelled: [],
      naming: [{ bucket, how: "direct", via: null, onEngines: true }],
      engines: [],
    });
    HW = {
      "user/u-ana": ok({
        principal: { kind: "user", id: "u-ana", label: "Ana", username: "ana" },
        state: "listed",
        policies: [row(1, "hw-access", "access", "allow"), row(2, "hw-mask", "masking", "mask"), row(3, "hw-filter", "rowfilter", "row_filter")],
        everyone: { count: 0, policies: [] },
        unreadable: [],
      }) as { status: number; body: unknown },
    };
    serve({ "users/u-ana": ok(ANA) }, ok([]));
    open("/governance/people/users/u-ana?tab=masks");
    const section = await within(await screen.findByRole("tabpanel")).findByRole("region", { name: "Hand-written policies (2)" });
    expect(within(section).getByText("hw-mask")).toBeInTheDocument();
    expect(within(section).getByText("hw-filter")).toBeInTheDocument();
    expect(within(section).queryByText("hw-access")).not.toBeInTheDocument();
  });
});
