import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type {
  ExplainPrincipalResponse,
  ExplainResponse,
  GroupPrincipal,
  RolePrincipal,
  UserPrincipal,
  WireDecision,
} from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import PeopleRoutes from "@/routes/governance/people/PeopleRoutes";

// Radix Select has no layout in jsdom; the picker's own behaviour is ScopePicker.test.tsx's.
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({ onChange }: { onChange: (v: unknown) => void }) => (
    <div>
      <button type="button" onClick={() => onChange({ catalog_id: "c-ice", schema: "sales" })}>
        set-scope
      </button>
      <button type="button" onClick={() => onChange({ catalog_id: "c-ice", schema: "hr" })}>
        set-other-scope
      </button>
    </div>
  ),
}));

type Answer = { status: number; body: unknown };
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const ok = (body: unknown): Answer => ({ status: 200, body });

type Fake = {
  principal?: Record<string, Answer>;
  explain?: Answer | (() => Promise<Answer>);
  explainPrincipal?: Answer | (() => Promise<Answer>);
  people?: Answer;
  principals?: Answer;
};

/** Stubs `fetch`; returns each perm POST made, with its body. */
function serve(fake: Fake) {
  const posts: { call: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const method = (init?.method ?? "GET").toUpperCase();
      const reply = (a: Answer | undefined) => Promise.resolve(a ? json(a.status, a.body) : json(404, { error: "not found" }));
      const answer = (a: Answer | (() => Promise<Answer>) | undefined) => (typeof a === "function" ? a().then(reply) : reply(a));
      const m = /^\/api\/control-plane\/perm\/people\/(users|groups|roles)\/([^/]+)$/.exec(url.pathname);
      if (m) return reply(fake.principal?.[`${m[1]}/${decodeURIComponent(m[2])}`]);
      if (url.pathname === "/api/control-plane/perm/people") return reply(fake.people ?? ok({ items: [], total: 0 }));
      if (url.pathname === "/api/control-plane/principals") return reply(fake.principals ?? ok([]));
      if (method === "POST" && url.pathname === "/api/control-plane/perm/explain") {
        posts.push({ call: "explain", body: JSON.parse(String(init?.body)) });
        return answer(fake.explain);
      }
      if (method === "POST" && url.pathname === "/api/control-plane/perm/explain/principal") {
        posts.push({ call: "explain/principal", body: JSON.parse(String(init?.body)) });
        return answer(fake.explainPrincipal);
      }
      return reply(undefined);
    }),
  );
  return posts;
}

const READER: Me = { user_id: "s1", email: "s@example.com", platform_roles: [], permissions: ["policy:read"] };

function open(url: string) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: READER, isLoading: false }}>
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

const ALLOWED: WireDecision = { kind: "allowed", policy_id: 7, policy_name: "perm-grant-1" };
const DENIED: WireDecision = { kind: "denied", policy_id: 8, policy_name: "perm-grant-2", reason: "deny_policy_item" };
const UNDECIDABLE: WireDecision = { kind: "undecidable", why: [] };

const explainOf = (...decisions: WireDecision[]): ExplainResponse => ({
  outcome: "per_profile",
  profiles: [
    {
      service_id: 11,
      service_name: "egret-trino-prod",
      engine: "trino",
      tags: [],
      tag_policies_in_download: 0,
      accesses: decisions.map((decision, i) => ({ access_type: i === 0 ? "select" : `access${i}`, decision })),
    },
  ],
});

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
  clearances: [],
};

const ANALYSTS: GroupPrincipal = {
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

const READER_ROLE: RolePrincipal = { name: "reader", scopes: ["platform"], holders: [], serviceAccounts: 0, holderGroups: [], standsFor: [], clearances: [] };

// One member allowed, one refused, one undecidable: any single word for the group would be wrong.
const MEMBERS: ExplainPrincipalResponse = {
  membersTotal: 4,
  limit: 200,
  truncated: false,
  evaluated: [
    { userId: "u-ana", label: "Ana Silva", username: "ana", result: explainOf(ALLOWED) },
    { userId: "u-bob", label: "Bob Ray", username: "bob", result: explainOf(DENIED) },
    { userId: "u-cy", label: "Cy Lee", username: "cy", result: explainOf(ALLOWED, UNDECIDABLE) },
  ],
  notEvaluated: [{ userId: "u-dee", label: "Dee Noname", reason: "no_username" }],
};

async function ask(panel: HTMLElement) {
  const user = userEvent.setup();
  await user.click(within(panel).getByRole("button", { name: "set-scope" }));
  await user.click(within(panel).getByRole("button", { name: "Explain" }));
  return user;
}

describe("Check access for a user", () => {
  it("asks /perm/explain by username and renders the per-profile answer", async () => {
    const posts = serve({ principal: { "users/u-ana": ok(ANA) }, explain: ok(explainOf(ALLOWED)) });
    open("/governance/people/users/u-ana?tab=check");
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByRole("button", { name: "Explain" })).toBeDisabled();
    expect(within(panel).getByText("Explain needs a target catalog.")).toBeInTheDocument();

    await ask(panel);

    const result = await within(panel).findByTestId("explain-result");
    expect(within(result).getByText("egret-trino-prod")).toBeInTheDocument();
    expect(posts).toEqual([
      { call: "explain", body: { subject: { user: "ana" }, action: "read", target: { catalog_id: "c-ice", schema: "sales" } } },
    ]);
  });

  it("asks nothing for a user with no username, and says why", async () => {
    const posts = serve({ principal: { "users/u-ana": ok({ ...ANA, username: null }) } });
    open("/governance/people/users/u-ana?tab=check");
    const panel = await screen.findByRole("tabpanel");
    expect(await within(panel).findByText("No username yet, so no engine knows this user.")).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Explain" })).not.toBeInTheDocument();
    expect(posts).toEqual([]);
  });

  it("shows an answer only under the question it was asked for", async () => {
    let answer: (a: Answer) => void = () => {};
    serve({ principal: { "users/u-ana": ok(ANA) }, explain: () => new Promise((r) => (answer = r)) });
    open("/governance/people/users/u-ana?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    const user = await ask(panel);
    // The target changes while the answer is on its way.
    await user.click(within(panel).getByRole("button", { name: "set-other-scope" }));
    answer(ok(explainOf(ALLOWED)));
    await waitFor(() => expect(within(panel).getByRole("button", { name: "Explain" })).toBeEnabled());
    expect(within(panel).queryByTestId("explain-result")).not.toBeInTheDocument();
  });
});

describe("Check access for a group or a role", () => {
  it("answers each member on their own, with the three counts and no single verdict", async () => {
    const posts = serve({ principal: { "groups/analysts": ok(ANALYSTS) }, explainPrincipal: ok(MEMBERS) });
    open("/governance/people/groups/analysts?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    await ask(panel);

    const view = await within(panel).findByTestId("members-explain");
    expect(posts).toEqual([
      {
        call: "explain/principal",
        body: { principal: { kind: "group", ref: "analysts" }, action: "read", target: { catalog_id: "c-ice", schema: "sales" } },
      },
    ]);
    const counts = Object.fromEntries(
      within(view)
        .getAllByRole("term")
        .map((dt) => [dt.textContent, dt.nextElementSibling?.textContent]),
    );
    expect(counts).toEqual({ "Allowed by every profile": "1", "Refused by at least one": "1", "Undecidable on at least one": "1" });

    const rows = within(within(view).getByRole("table", { name: "Members" })).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[1].textContent)).toEqual([
      "egret-trino-prod: select Allowed",
      "egret-trino-prod: select Refused by a deny item",
      "egret-trino-prod: select Allowed, access1 Undecidable",
    ]);
    // No verdict for the group as a whole, and no per-profile answer until a member is expanded.
    expect(within(view).queryByTestId("explain-result")).not.toBeInTheDocument();
    expect(within(view).queryByTestId("decision-badge")).not.toBeInTheDocument();
    expect(within(view).getByText(/so there is no single verdict/)).toBeInTheDocument();

    const notAsked = within(view).getByRole("region", { name: "Not asked" });
    expect(notAsked).toHaveTextContent("Dee Noname: No username yet, so no engine knows this user.");
    expect(within(view).queryByText(/the others were not asked/)).not.toBeInTheDocument();
  });

  it("filters to refused members, and opens a member's full answer", async () => {
    serve({ principal: { "groups/analysts": ok(ANALYSTS) }, explainPrincipal: ok(MEMBERS) });
    open("/governance/people/groups/analysts?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    const user = await ask(panel);
    const view = await within(panel).findByTestId("members-explain");

    await user.click(within(view).getByRole("switch", { name: "Refused only" }));
    const table = within(view).getByRole("table", { name: "Members" });
    expect(within(table).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["Bob Raybob"]);

    const more = within(table).getByRole("button", { name: "Show the full answer for Bob Ray" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    await user.click(more);
    const full = within(view).getByTestId("explain-result");
    expect(within(full).getByText("Refused by a deny item")).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "Hide the full answer for Bob Ray" })).toHaveAttribute("aria-expanded", "true");
  });

  it("says a refused-only filter that matches nobody, and clears it", async () => {
    serve({
      principal: { "groups/analysts": ok(ANALYSTS) },
      explainPrincipal: ok({ ...MEMBERS, evaluated: [MEMBERS.evaluated[0]], membersTotal: 1, notEvaluated: [] }),
    });
    open("/governance/people/groups/analysts?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    const user = await ask(panel);
    const view = await within(panel).findByTestId("members-explain");
    await user.click(within(view).getByRole("switch", { name: "Refused only" }));
    expect(within(view).getByText("No evaluated member is refused.")).toBeInTheDocument();
    await user.click(within(view).getByRole("button", { name: "Show every member" }));
    expect(within(view).getByRole("table", { name: "Members" })).toBeInTheDocument();
  });

  it("says how many were evaluated when the server stopped at its limit", async () => {
    serve({
      principal: { "roles/reader": ok(READER_ROLE) },
      explainPrincipal: ok({ ...MEMBERS, membersTotal: 201, truncated: true, notEvaluated: [] }),
    });
    open("/governance/people/roles/reader?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    await ask(panel);
    expect(await within(panel).findByText("Evaluated 3 of 201 members; the others were not asked.")).toBeInTheDocument();
  });

  it("shows the server's sentence when it refuses the question", async () => {
    serve({
      principal: { "roles/reader": ok(READER_ROLE) },
      explainPrincipal: { status: 400, body: { error: "No role named reader." } },
    });
    open("/governance/people/roles/reader?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    await ask(panel);
    expect(await within(panel).findByText("No role named reader.")).toBeInTheDocument();
  });

  it("says nobody was asked for a group with no members", async () => {
    serve({
      principal: { "groups/analysts": ok(ANALYSTS) },
      explainPrincipal: ok({ membersTotal: 0, limit: 200, truncated: false, evaluated: [], notEvaluated: [] }),
    });
    open("/governance/people/groups/analysts?tab=check");
    const panel = await screen.findByRole("tabpanel");
    await within(panel).findByRole("button", { name: "Explain" });
    await ask(panel);
    expect(await within(panel).findByText("This group has no members, so nobody was asked.")).toBeInTheDocument();
  });
});

describe("the index's Check access dialog", () => {
  it("asks about a group picked by kind and name, rendered as a principal page renders it", async () => {
    const posts = serve({
      people: ok({ items: [{ kind: "group", key: "analysts", label: "analysts", detail: "local", serviceAccount: false }], total: 1 }),
      explainPrincipal: ok(MEMBERS),
    });
    const user = userEvent.setup();
    open("/governance/people?kind=group");
    await user.click(await screen.findByRole("button", { name: "Check access" }));
    const dialog = await screen.findByRole("dialog", { name: "Check access" });
    expect(within(dialog).getByText("Explain needs a user and a target catalog.")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("combobox", { name: "Subject" }));
    await user.click(await screen.findByRole("option", { name: "Group" }));
    await user.click(within(dialog).getByLabelText("Group"));
    await user.click(await within(dialog).findByRole("button", { name: /analysts/ }));
    expect(within(dialog).getByLabelText("Group")).toHaveValue("analysts");

    await user.click(within(dialog).getByRole("combobox", { name: "Action" }));
    await user.click(await screen.findByRole("option", { name: "write" }));
    await user.click(within(dialog).getByRole("button", { name: "set-scope" }));
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByTestId("members-explain")).toBeInTheDocument();
    expect(posts).toEqual([
      {
        call: "explain/principal",
        body: { principal: { kind: "group", ref: "analysts" }, action: "write", target: { catalog_id: "c-ice", schema: "sales" } },
      },
    ]);
  });

  it("asks about a user through /perm/explain", async () => {
    const posts = serve({ explain: ok(explainOf(ALLOWED)) });
    const user = userEvent.setup();
    open("/governance/people");
    await user.click(await screen.findByRole("button", { name: "Check access" }));
    const dialog = await screen.findByRole("dialog", { name: "Check access" });
    await user.type(within(dialog).getByLabelText("User"), "ana");
    await user.click(within(dialog).getByRole("button", { name: "set-scope" }));
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));
    expect(await within(dialog).findByTestId("explain-result")).toBeInTheDocument();
    expect(posts.map((p) => p.call)).toEqual(["explain"]);

    // A changed subject hides the answer that was for another.
    await user.type(within(dialog).getByLabelText("User"), "2");
    expect(within(dialog).queryByTestId("explain-result")).not.toBeInTheDocument();
  });
});
