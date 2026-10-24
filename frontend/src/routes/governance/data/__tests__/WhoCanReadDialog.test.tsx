import { afterEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import type { ExplainPrincipalResponse, ExplainResponse } from "@/api/generated/model";
import { everyRowLine, ruleMaskLine } from "@/components/perm/__tests__/effectLineFixtures";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WhoCanReadDialog } from "@/routes/governance/data/WhoCanReadDialog";

/*
 * "Who can read?" as the live walkthrough met it (Task 12b §3, §5, §7;
 * findings 3, 4; N18, N22): a typed user nobody has, a typed group nobody
 * has, a group on a catalog nothing enforces, the Explain button's reason, and
 * the suggestions at 390 px.
 */

type Answer = { status: number; body: unknown };
const json = (a: Answer) => new Response(JSON.stringify(a.body), { status: a.status, headers: { "content-type": "application/json" } });
const ok = (body: unknown): Answer => ({ status: 200, body });

type Fake = {
  /** `GET …/perm/people?kind=user&username=…`, by username; absent: one user with that username. */
  userLookup?: (username: string) => Answer;
  principals?: Answer;
  explain?: Answer;
  explainPrincipal?: Answer;
};

function serve(fake: Fake) {
  const calls: { method: string; path: string; search: URLSearchParams; body?: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const method = (init?.method ?? "GET").toUpperCase();
      calls.push({ method, path: url.pathname, search: url.searchParams, body: init?.body ? JSON.parse(String(init.body)) : undefined });
      if (url.pathname === "/api/control-plane/perm/people") {
        const username = url.searchParams.get("username");
        if (username !== null) {
          return Promise.resolve(
            json(
              fake.userLookup?.(username) ??
                ok({ items: [{ kind: "user", key: `u-${username}`, label: username, username, serviceAccount: false }], total: 1 }),
            ),
          );
        }
        return Promise.resolve(json(ok({ items: [], total: 0 })));
      }
      if (url.pathname === "/api/control-plane/principals") return Promise.resolve(json(fake.principals ?? ok([])));
      if (method === "POST" && url.pathname === "/api/control-plane/perm/explain") return Promise.resolve(json(fake.explain ?? ok(PER_PROFILE)));
      if (method === "POST" && url.pathname === "/api/control-plane/perm/explain/principal")
        return Promise.resolve(json(fake.explainPrincipal ?? ok(MEMBERS_NONE)));
      return Promise.resolve(json({ status: 404, body: { error: "not found" } }));
    }),
  );
  const posts = () => calls.filter((c) => c.method === "POST");
  return { calls, posts };
}

const PER_PROFILE: ExplainResponse = {
  outcome: "per_profile",
  profiles: [
    {
      service_id: 11,
      service_name: "egret-trino-prod",
      engine: "trino",
      tags: [],
      tag_policies_in_download: 0,
      effect_lines: [],
      accesses: [{ access_type: "select", decision: { kind: "allowed", policy_id: 7, policy_name: "perm-grant-1" } }],
    },
  ],
};
const NO_PROFILE: ExplainResponse = { outcome: "no_profile_serves_the_catalog", profiles: [] };
const MEMBERS_NONE: ExplainPrincipalResponse = { membersTotal: 0, limit: 200, truncated: false, evaluated: [], notEvaluated: [] };

function open() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      {/* The dialog opens inside the Data area's routes; its answer links to rules and people. */}
      <MemoryRouter>
        <TooltipProvider>
          <WhoCanReadDialog
            open
            onOpenChange={() => {}}
            path={{ catalog: "demo_warehouse", schema: "analytics", object: "customers", column: "email" }}
            catalogId="c-demo"
          />
        </TooltipProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

async function pickKind(user: ReturnType<typeof userEvent.setup>, dialog: HTMLElement, name: "Group" | "Role") {
  await user.click(within(dialog).getByRole("combobox", { name: "Subject" }));
  await user.click(await screen.findByRole("option", { name }));
}

afterEach(() => vi.unstubAllGlobals());

describe("Who can read? — a typed user is checked against the known users (finding 3)", () => {
  it("says no user has that username, and asks nothing", async () => {
    const { calls, posts } = serve({ userLookup: () => ok({ items: [], total: 0 }) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "admin");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByText("No user named admin. Pick a user from the suggestions.")).toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/control-plane/perm/people" && c.search.get("username") === "admin")).toBe(true);
    expect(posts()).toHaveLength(0);
    expect(within(dialog).queryByTestId("explain-result")).not.toBeInTheDocument();
  });

  it("asks explain when the username is known", async () => {
    const { posts } = serve({});
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "ana");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByTestId("explain-result")).toBeInTheDocument();
    expect(posts()[0].body).toEqual({
      subject: { user: "ana" },
      action: "read",
      target: { catalog_id: "c-demo", schema: "analytics", table: "customers", column: "email" },
    });
    expect(within(dialog).queryByText(/No user named/)).not.toBeInTheDocument();
  });

  it("asks explain anyway when the lookup cannot be made, and never claims there is no such user", async () => {
    const { posts } = serve({ userLookup: () => ({ status: 403, body: { error: "forbidden" } }) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "ana");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByTestId("explain-result")).toBeInTheDocument();
    expect(posts()).toHaveLength(1);
    expect(within(dialog).queryByText(/No user named/)).not.toBeInTheDocument();
  });
});

describe("Who can read? — the answer leads with the server's sentence (Package 2, spec §5.10)", () => {
  it("shows a user's line per engine before the folded verdicts", async () => {
    const answer: ExplainResponse = {
      ...PER_PROFILE,
      profiles: [
        {
          ...PER_PROFILE.profiles[0],
          effect_lines: [ruleMaskLine("ana", "email", "hashed", "Hash personal data", "r-hash"), everyRowLine("ana")],
        },
      ],
    };
    serve({ explain: ok(answer) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "ana");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const profile = await within(dialog).findByTestId("explain-profile");
    const [first] = within(profile).getAllByTestId("effect-line");
    expect(first).toHaveTextContent("ana reads email as hashed (rule Hash personal data)");
    expect(first).toBeVisible();
    expect(within(first).getByRole("link", { name: "Hash personal data" })).toHaveAttribute("href", "/governance/rules/r-hash");
    expect(within(profile).getByTestId("explain-access")).not.toBeVisible();
  });
});

describe("Who can read? — member names wrap only where they must (live check, 390 px)", () => {
  it("does not break a member's username mid-word: only an unbreakable token too long for the cell breaks", async () => {
    serve({
      explainPrincipal: ok({
        membersTotal: 1,
        limit: 200,
        truncated: false,
        evaluated: [{ userId: "u-g", label: "Exempt G", username: "exempt-g", result: PER_PROFILE }],
        notEvaluated: [],
      } satisfies ExplainPrincipalResponse),
    });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await pickKind(user, dialog, "Group");
    await user.type(within(dialog).getByRole("combobox", { name: "Group" }), "data-team");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const view = await within(dialog).findByTestId("members-explain");
    // jsdom has no layout: this pins the wrapping class; the painted result is a browser check.
    const name = within(view).getByText("exempt-g");
    expect(name.className).not.toMatch(/\bbreak-all\b/);
    expect(name.className).toMatch(/\bbreak-words\b/);
  });
});

describe("Who can read? — a group nobody has (finding 3, §7)", () => {
  it("says so in a sentence, never the server's raw refusal", async () => {
    serve({ explainPrincipal: { status: 400, body: { error: "bad request: no group is named data" } } });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await pickKind(user, dialog, "Group");
    await user.type(within(dialog).getByRole("combobox", { name: "Group" }), "data");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByText("No group named data. Pick a group from the suggestions.")).toBeInTheDocument();
    expect(within(dialog).queryByText(/bad request/)).not.toBeInTheDocument();
  });

  it("says so for a role as well", async () => {
    serve({ explainPrincipal: { status: 400, body: { error: "bad request: no role is named reeder" } } });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await pickKind(user, dialog, "Role");
    await user.type(within(dialog).getByRole("combobox", { name: "Role" }), "reeder");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    expect(await within(dialog).findByText("No role named reeder. Pick a role from the suggestions.")).toBeInTheDocument();
  });
});

describe("Who can read? — a group on a catalog nothing enforces (finding 4)", () => {
  it("says nothing is enforcing, as user mode does, instead of four zero counts", async () => {
    serve({
      explainPrincipal: ok({
        membersTotal: 2,
        limit: 200,
        truncated: false,
        evaluated: [{ userId: "u-a", label: "Ana", username: "ana", result: NO_PROFILE }],
        notEvaluated: [{ userId: "u-b", label: "Member SSO", reason: "no_username" }],
      } satisfies ExplainPrincipalResponse),
    });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await pickKind(user, dialog, "Group");
    await user.type(within(dialog).getByRole("combobox", { name: "Group" }), "data-team");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const view = await within(dialog).findByTestId("members-explain");
    const state = within(view).getByTestId("no-profile-state");
    expect(state).toHaveTextContent("Not enforced");
    expect(state).toHaveTextContent(
      "No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it.",
    );
    expect(within(view).queryByText("Members allowed by every profile")).not.toBeInTheDocument();
    expect(within(view).getByRole("region", { name: "Not asked" })).toHaveTextContent("Member SSO");
  });

  it("shows no counts when no member could be asked", async () => {
    serve({
      explainPrincipal: ok({
        membersTotal: 1,
        limit: 200,
        truncated: false,
        evaluated: [],
        notEvaluated: [{ userId: "u-b", label: "Member SSO", reason: "no_username" }],
      } satisfies ExplainPrincipalResponse),
    });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await pickKind(user, dialog, "Group");
    await user.type(within(dialog).getByRole("combobox", { name: "Group" }), "data-team");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const view = await within(dialog).findByTestId("members-explain");
    expect(within(view).queryByText("Members allowed by every profile")).not.toBeInTheDocument();
    expect(view).toHaveTextContent("No member could be asked: none has a username yet, so there is no answer here.");
    expect(within(view).getByRole("region", { name: "Not asked" })).toHaveTextContent("Member SSO");
  });

  it("speaks of engines, not of policy profiles, in the subtitles (N18)", async () => {
    serve({
      explainPrincipal: ok({
        membersTotal: 1,
        limit: 200,
        truncated: false,
        evaluated: [{ userId: "u-a", label: "Ana", username: "ana", result: PER_PROFILE }],
        notEvaluated: [],
      } satisfies ExplainPrincipalResponse),
    });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    expect(dialog).toHaveTextContent("as each engine that serves its catalog would decide it");
    expect(dialog).not.toHaveTextContent(/per policy profile/);
    await pickKind(user, dialog, "Group");
    await user.type(within(dialog).getByRole("combobox", { name: "Group" }), "data-team");
    await user.click(within(dialog).getByRole("button", { name: "Explain" }));

    const view = await within(dialog).findByTestId("members-explain");
    expect(view).toHaveTextContent("Each member's own answer, as each engine that serves this catalog would decide it.");
    expect(view).not.toHaveTextContent(/per policy profile/);
  });
});

describe("Who can read? — Explain says why it is blocked and stays focusable (N22)", () => {
  it("is aria-disabled, described by its reason, reachable by Tab, and does nothing while blocked", async () => {
    const { posts } = serve({});
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    const explain = within(dialog).getByRole("button", { name: "Explain" });
    expect(explain).not.toBeDisabled();
    expect(explain).toHaveAttribute("aria-disabled", "true");
    expect(explain).toHaveAccessibleDescription("Explain needs a user.");
    explain.focus();
    expect(explain).toHaveFocus();
    await user.click(explain);
    await user.keyboard("{Enter}");
    expect(posts()).toHaveLength(0);

    await user.type(within(dialog).getByLabelText("User"), "ana");
    expect(explain).not.toHaveAttribute("aria-disabled", "true");
    expect(explain).not.toHaveAccessibleDescription();
  });
});

describe("Who can read? — the user suggestions sit in the flow, not over Explain (§5, finding 3)", () => {
  it("renders the suggestion list in flow, between the field and Explain", async () => {
    serve({ principals: ok([{ kind: "user", name: "ana@example.com", label: "Ana", username: "ana" }]) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "an");
    const list = await within(dialog).findByRole("listbox", { name: "User suggestions" });
    await within(list).findByRole("option", { name: /Ana/ });
    // jsdom has no layout: this pins the class that takes the popup out of the
    // flow; the painted result needs a browser check (FE-TEST-2).
    expect(list.parentElement?.className ?? "").not.toMatch(/\babsolute\b/);
    const explain = within(dialog).getByRole("button", { name: "Explain" });
    expect(list.compareDocumentPosition(explain) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("Who can read? — one click on Explain runs Explain while the suggestions are open (live check D1)", () => {
  it("keeps the list's space until the click has landed, so the click reaches Explain", async () => {
    const { calls } = serve({ userLookup: () => ok({ items: [], total: 0 }) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    const field = within(dialog).getByLabelText("User");
    await user.type(field, "nobody");
    expect(await within(dialog).findByText("No matches.")).toBeInTheDocument();
    const explain = within(dialog).getByRole("button", { name: "Explain" });

    // The browser's order for one click: pointerdown, mousedown and the focus
    // move (the field's blur), then pointerup, mouseup and click. jsdom has no
    // layout, so the hit test is modelled: the in-flow list sits above Explain,
    // and if it has collapsed before the button is released, Explain has moved
    // up from under the pointer and the click lands on the dialog instead.
    fireEvent.pointerDown(explain);
    fireEvent.mouseDown(explain);
    act(() => explain.focus());
    const listStillThere = within(dialog).queryByText("No matches.") !== null;
    const target = listStillThere ? explain : dialog;
    fireEvent.pointerUp(target);
    fireEvent.mouseUp(target);
    fireEvent.click(target);

    const lookups = () => calls.filter((c) => c.path === "/api/control-plane/perm/people" && c.search.get("username") === "nobody");
    await waitFor(() => expect(lookups()).toHaveLength(1));
    expect(await within(dialog).findByText("No user named nobody. Pick a user from the suggestions.")).toBeInTheDocument();
    // Once the click has landed, the list closes: focus has left the field.
    await waitFor(() => expect(within(dialog).queryByText("No matches.")).not.toBeInTheDocument());
    expect(lookups()).toHaveLength(1);
  });

  it("closes the list when Explain runs from the field (Enter), so it does not hang under the answer (live check)", async () => {
    serve({ userLookup: () => ok({ items: [], total: 0 }) });
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "nobody");
    expect(await within(dialog).findByText("No matches.")).toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(await within(dialog).findByText("No user named nobody. Pick a user from the suggestions.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("closes the list when focus leaves by keyboard", async () => {
    serve({});
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    await user.type(within(dialog).getByLabelText("User"), "nobody");
    expect(await within(dialog).findByText("No matches.")).toBeInTheDocument();
    await user.tab();
    await waitFor(() => expect(within(dialog).queryByText("No matches.")).not.toBeInTheDocument());
  });

  it("Tab from the field with the list open lands on Explain, not on the dialog (live check N1)", async () => {
    serve({});
    const user = userEvent.setup();
    open();
    const dialog = screen.getByRole("dialog", { name: "Who can read?" });
    const field = within(dialog).getByLabelText("User");
    await user.type(field, "nobody");
    expect(await within(dialog).findByText("No matches.")).toBeInTheDocument();
    const explain = within(dialog).getByRole("button", { name: "Explain" });

    // The browser's order for Tab: the field's focusout runs while <body>
    // holds focus, the microtasks after it run, and only then does Explain get
    // its focusin. The dialog's FocusScope watches for removed nodes in those
    // microtasks and, finding <body> focused, pulls focus to its container.
    act(() => field.blur());
    await act(async () => {
      await new Promise<void>((r) => queueMicrotask(r));
      await new Promise<void>((r) => queueMicrotask(r));
    });
    expect(document.activeElement).toBe(document.body);
    act(() => explain.focus());

    await waitFor(() => expect(within(dialog).queryByText("No matches.")).not.toBeInTheDocument());
    expect(document.activeElement).toBe(explain);
  });
});
