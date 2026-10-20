import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { ClearanceFact, GroupPrincipal, RolePrincipal, UserPrincipal } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";

type Answer = { status: number; body: unknown };
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const ok = (body: unknown): Answer => ({ status: 200, body });

/** Stubs `fetch` for a principal read; everything else is a 404. */
function serve(principal: Record<string, Answer>) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const m = /^\/api\/control-plane\/perm\/people\/(users|groups|roles)\/([^/]+)$/.exec(url.pathname);
      const a = m ? principal[`${m[1]}/${decodeURIComponent(m[2])}`] : undefined;
      return Promise.resolve(a ? json(a.status, a.body) : json(404, { error: "not found" }));
    }),
  );
}

const me = (permissions: string[]): Me => ({ user_id: "s1", email: "s@example.com", platform_roles: [], permissions });
const READER = me(["policy:read"]);
const TAG_READER = me(["policy:read", "tag:read"]);

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

const WS = "6f1c2a90-0000-4000-8000-00000000000a";

const fact = (tagId: string, tagName: string, over: Partial<ClearanceFact> & Pick<ClearanceFact, "engines" | "egret">): ClearanceFact => ({
  tagId,
  tagKey: tagName.toLowerCase(),
  tagName,
  requiredRoles: [],
  requiredGroups: [],
  ...over,
});

const CLEARANCES: ClearanceFact[] = [
  fact("t-pii", "PII", {
    requiredRoles: ["pii_reader"],
    engines: { kind: "cleared", role: "pii_reader" },
    egret: { kind: "cleared", via: { kind: "role", name: "pii_reader" }, scope: { type: "platform" } },
  }),
  fact("t-hr", "HR", {
    requiredGroups: ["hr"],
    engines: { kind: "not_enforced_groups_only" },
    egret: { kind: "cleared", via: { kind: "group", name: "hr" } },
  }),
  fact("t-fin", "Finance", {
    requiredRoles: ["fin_reader"],
    engines: { kind: "not_cleared", because: "held_through_group" },
    egret: {
      kind: "cleared",
      via: { kind: "group_role", group: "analysts", role: "fin_reader" },
      scope: { type: "workspace", id: WS, name: "Sales" },
    },
  }),
  fact("t-sec", "Secret", {
    requiredRoles: ["sec"],
    requiredGroups: ["sec_team"],
    engines: { kind: "not_cleared", because: "groups_not_enforced" },
    egret: { kind: "cleared", via: { kind: "group", name: "sec_team" } },
  }),
  fact("t-top", "Top", {
    requiredRoles: ["top"],
    engines: { kind: "not_cleared" },
    egret: { kind: "not_cleared" },
  }),
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
  standsFor: [],
  clearances: CLEARANCES,
};

const rowOf = (tag: string) => {
  const table = within(screen.getByRole("tabpanel")).getByRole("table", { name: "Clearances" });
  const row = within(table)
    .getAllByRole("row")
    .find((r) => within(r).queryAllByRole("cell")[0]?.textContent?.startsWith(tag));
  if (!row) throw new Error(`no row for ${tag}`);
  return within(row).getAllByRole("cell").map((c) => c.textContent);
};

describe("a principal's Clearances tab", () => {
  it("answers each gated tag for engines and for egret objects, in words", async () => {
    serve({ "users/u-ana": ok(ANA) });
    open("/governance/people/users/u-ana?tab=clearances");

    const panel = await screen.findByRole("tabpanel");
    const table = await within(panel).findByRole("table", { name: "Clearances" });
    expect(within(table).getAllByRole("row")).toHaveLength(1 + CLEARANCES.length);

    expect(rowOf("PII")).toEqual(["PII", "Roles: pii_reader", "Cleared through role pii_reader", "Cleared through role pii_reader"]);
    expect(rowOf("HR")).toEqual([
      "HR",
      "Groups: hr",
      "Engines do not enforce this gate (it names only groups)",
      "Cleared through group hr",
    ]);
    expect(rowOf("Finance")).toEqual([
      "Finance",
      "Roles: fin_reader",
      "Not cleared: the required role is held through a group, and engines count only roles held directly",
      "Cleared through role fin_reader, held by group analysts, for workspace Sales's objects only: a workspace role clears only that workspace's objects",
    ]);
    expect(rowOf("Secret")).toEqual([
      "Secret",
      "Roles: secGroups: sec_team",
      "Not cleared: engines enforce only the gate's roles, not its groups",
      "Cleared through group sec_team",
    ]);
    expect(rowOf("Top")).toEqual(["Top", "Roles: top", "Not cleared", "Not cleared"]);
  });

  it("links a tag to its classification page only with tag:read", async () => {
    serve({ "users/u-ana": ok(ANA) });
    const { unmount } = open("/governance/people/users/u-ana?tab=clearances", TAG_READER);
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByRole("link", { name: "PII" })).toHaveAttribute("href", "/governance/classifications/t-pii?tab=clearance");
    unmount();

    open("/governance/people/users/u-ana?tab=clearances", READER);
    const again = await screen.findByRole("tabpanel");
    expect(await within(again).findByRole("table", { name: "Clearances" })).toBeInTheDocument();
    expect(within(again).getByText("PII")).toBeInTheDocument();
    expect(within(again).queryByRole("link", { name: "PII" })).not.toBeInTheDocument();
  });

  it("says why engines do not clear: expired or no username", async () => {
    serve({
      "users/u-ana": ok({
        ...ANA,
        clearances: [
          fact("t-a", "Alpha", { requiredRoles: ["a"], engines: { kind: "not_cleared", because: "expired" }, egret: { kind: "not_cleared" } }),
          fact("t-b", "Beta", {
            requiredRoles: ["b"],
            engines: { kind: "not_cleared", because: "no_username" },
            egret: { kind: "cleared", via: { kind: "role", name: "b" }, scope: { type: "platform" } },
          }),
        ],
      }),
    });
    open("/governance/people/users/u-ana?tab=clearances");

    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Clearances" });
    expect(rowOf("Alpha")[2]).toBe("Not cleared: the required role has expired. Engines may apply it until the next recompile.");
    expect(rowOf("Beta")[2]).toBe("Not cleared: no username yet");
  });

  it("says a platform admin is exempt, and how far an object role clears", async () => {
    serve({
      "users/u-ana": ok({
        ...ANA,
        clearances: [
          fact("t-a", "Alpha", {
            requiredRoles: ["a"],
            engines: { kind: "not_cleared" },
            egret: { kind: "cleared", via: { kind: "role", name: "platform_admin" }, scope: { type: "platform" } },
          }),
          fact("t-b", "Beta", {
            requiredRoles: ["b"],
            engines: { kind: "cleared", role: "b" },
            egret: { kind: "cleared", via: { kind: "role", name: "b" }, scope: { type: "object", id: "f-1", objectType: "folder" } },
          }),
        ],
      }),
    });
    open("/governance/people/users/u-ana?tab=clearances");

    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Clearances" });
    expect(rowOf("Alpha")[3]).toBe("Cleared through role platform_admin (platform admins are exempt from every gate)");
    expect(rowOf("Beta")[3]).toBe("Cleared through role b, for folder f-1 only: an object role clears only that object");
  });

  it("states the fact when no classification is gated", async () => {
    serve({ "users/u-ana": ok({ ...ANA, clearances: [] }) });
    open("/governance/people/users/u-ana?tab=clearances");

    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("No classification is gated")).toBeInTheDocument();
    expect(within(panel).queryByRole("table")).not.toBeInTheDocument();
  });

  it("answers for a group and a role the gate names itself", async () => {
    const analysts: GroupPrincipal = {
      name: "hr",
      source: "local",
      isPublic: false,
      members: [],
      serviceAccounts: 0,
      roles: [],
      workspaces: [],
      standsFor: [],
      clearances: [
        fact("t-hr", "HR", {
          requiredRoles: ["hr_reader"],
          requiredGroups: ["hr"],
          engines: { kind: "not_cleared", because: "groups_not_enforced" },
          egret: { kind: "cleared", via: { kind: "itself" } },
        }),
      ],
    };
    const role: RolePrincipal = {
      name: "hr_reader",
      scopes: ["platform"],
      holders: [],
      serviceAccounts: 0,
      holderGroups: [],
      standsFor: [],
      clearances: [
        fact("t-hr", "HR", {
          requiredRoles: ["hr_reader"],
          requiredGroups: ["hr"],
          engines: { kind: "cleared", role: "hr_reader" },
          egret: { kind: "cleared", via: { kind: "itself" } },
        }),
      ],
    };
    serve({ "groups/hr": ok(analysts), "roles/hr_reader": ok(role) });

    const { unmount } = open("/governance/people/groups/hr?tab=clearances");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Clearances" });
    expect(rowOf("HR")[3]).toBe("Cleared: the gate names this group");
    unmount();

    open("/governance/people/roles/hr_reader?tab=clearances");
    await within(await screen.findByRole("tabpanel")).findByRole("table", { name: "Clearances" });
    expect(rowOf("HR")[2]).toBe("Cleared through role hr_reader");
    expect(rowOf("HR")[3]).toBe("Cleared: the gate names this role");
  });
});
