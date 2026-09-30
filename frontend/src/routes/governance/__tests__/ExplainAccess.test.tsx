import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * `ExplainAccess` reads the generated perm client directly (`usePermExplain`),
 * matching `Catalogs.test.tsx`'s pattern for the newest list pages. `ScopePicker`
 * is mocked to a dumb controlled stand-in — its own behavior (progressive
 * narrowing, row-filter mode) is covered by `ScopePicker.test.tsx`.
 *
 * **SP4d-2a Task 8.** `explain` no longer evaluates egret's own model: it
 * evaluates the policy set each engine actually downloads and answers once per
 * PROFILE, three-valued. So this file is about that shape — two profiles are two
 * results, "no profile serves this catalog" is its own state and not a denial, an
 * `Undecidable` names every cause it carries, and a policy egret's compiler does
 * not own is reported rather than dropped.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/policy-admin/policy-admin");
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) => (
    <div>
      <div data-testid="scope-value">{JSON.stringify(value)}</div>
      <button type="button" onClick={() => onChange({ catalog_id: "cat-1" })}>
        set-scope
      </button>
    </div>
  ),
}));

import * as permApi from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import { ApiError } from "@/api/client";
import type { ExplainResponse } from "@/api/generated/model";
import ExplainAccess from "@/routes/governance/ExplainAccess";

/**
 * Two profiles serving one catalog — the ordinary case on a cluster running
 * both engines, and the one a single aggregate verdict cannot describe. The
 * StarRocks profile also carries TWO access types for one `write`, which is
 * why they are never collapsed.
 */
const TWO_PROFILES: ExplainResponse = {
  outcome: "per_profile",
  profiles: [
    {
      service_id: 11,
      service_name: "egret-trino-prod",
      engine: "trino",
      accesses: [
        {
          access_type: "select",
          decision: {
            kind: "allowed",
            policy_id: 7,
            policy_name: "perm-grant-11111111-1111-4111-8111-111111111111",
          },
          attribution: { kind: "grant", grant_id: "grant-1" },
        },
      ],
    },
    {
      service_id: 12,
      service_name: "egret-starrocks-prod",
      engine: "starrocks",
      accesses: [
        {
          access_type: "insert",
          decision: {
            kind: "denied",
            policy_id: 9,
            policy_name: "perm-rule-22222222-2222-4222-8222-222222222222-sales.public.orders.ssn",
            reason: "deny_policy_item",
          },
          attribution: { kind: "rule", rule_id: "rule-1", locator: "sales.public.orders.ssn" },
        },
        {
          access_type: "delete",
          decision: { kind: "denied", policy_id: null, policy_name: "unnamed", reason: "no_policy_matched" },
          attribution: null,
        },
      ],
    },
  ],
  tags: ["pii"],
};

let mutateAsync: ReturnType<typeof vi.fn>;

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter>
      <QueryClientProvider client={qc}>
        <ExplainAccess />
      </QueryClientProvider>
    </MemoryRouter>,
  );
}

async function submit(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("User"), "alice-doe");
  await user.click(screen.getByRole("button", { name: "set-scope" }));
  await user.click(screen.getByRole("button", { name: /Explain/ }));
}

beforeEach(() => {
  vi.clearAllMocks();
  mutateAsync = vi.fn().mockResolvedValue({ status: 200, data: TWO_PROFILES });
  vi.mocked(permApi.usePermExplain).mockReturnValue({
    mutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermExplain>);
  vi.mocked(policyAdminListPrincipals).mockResolvedValue({
    status: 200,
    data: [],
    headers: new Headers(),
  } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);
});

describe("ExplainAccess — one answer per profile, over the artifact the engine downloads", () => {
  /** Item 1. Two profiles are two results, each naming itself and its engine. */
  test("two profiles render as two results, each naming its profile and engine", async () => {
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    await screen.findByTestId("explain-result");
    const profiles = screen.getAllByTestId("explain-profile");
    expect(profiles).toHaveLength(2);

    expect(profiles[0]).toHaveTextContent("egret-trino-prod");
    expect(profiles[0]).toHaveTextContent("Trino");
    expect(profiles[1]).toHaveTextContent("egret-starrocks-prod");
    expect(profiles[1]).toHaveTextContent("StarRocks");

    // Every access type keeps its own verdict: the StarRocks profile answers
    // `insert` and `delete` separately, because Ranger has no `write`.
    expect(within(profiles[0]).getAllByTestId("explain-access")).toHaveLength(1);
    const srAccesses = within(profiles[1]).getAllByTestId("explain-access");
    expect(srAccesses).toHaveLength(2);
    expect(srAccesses[0]).toHaveTextContent("insert");
    expect(srAccesses[1]).toHaveTextContent("delete");
  });

  /**
   * Item 1's other half: the old single summary badge is REMOVED, not
   * recoloured. There is no honest aggregate over {allowed, denied,
   * undecidable} × N profiles, and one badge reading "Allowed" above a profile
   * that refuses would be the most misleading pixel on the page.
   */
  test("no aggregate verdict is claimed above the per-profile answers", async () => {
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    expect(result).not.toHaveTextContent(/only a summary/i);
    expect(result).not.toHaveTextContent("Not allowed");
    // One decision badge per access, and not one more.
    expect(screen.getAllByTestId("decision-badge")).toHaveLength(3);
  });

  /** Item 1's deep links, which now live on the per-profile rows. */
  test("a grant attribution links to that grant, and a rule attribution to that rule", async () => {
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    await screen.findByTestId("explain-result");

    expect(screen.getByRole("link", { name: "View grant grant-1" })).toHaveAttribute(
      "href",
      "/governance/data-permissions?grant=grant-1",
    );
    const ruleLink = screen.getByRole("link", { name: "View rule rule-1" });
    expect(ruleLink).toHaveAttribute("href", "/governance/mask-filter-rules?rule=rule-1");
    // A per-column StarRocks rule expands ONE row into N policies; the locator
    // is the only thing that says which expansion decided.
    expect(screen.getByTestId("rule-locator")).toHaveTextContent("sales.public.orders.ssn");
  });

  /** Item 2. Nothing enforcing is a different fact from something refusing. */
  test("a catalog no profile serves is its own state, not a denial", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { outcome: "no_profile_serves_the_catalog", profiles: [], tags: [] },
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    const state = within(result).getByTestId("no-profile-state");
    expect(state).toHaveTextContent(/nothing is enforcing/i);
    expect(state).toHaveTextContent(/not a denial/i);

    // It must not borrow a denial's vocabulary or its colour.
    expect(screen.queryAllByTestId("explain-profile")).toHaveLength(0);
    expect(screen.queryAllByTestId("decision-badge")).toHaveLength(0);
    expect(result).not.toHaveTextContent("Denied");
  });

  /**
   * Item 3. `why` is a Vec: a profile can carry several causes, and collapsing
   * it to the first throws away exactly what the three-valued contract exists
   * to say. `Unmodelled::TagPolicies` carries its count as a JSON number, so
   * it renders as a sentence with a number in it, never a Debug string.
   */
  test("an undecidable verdict names every cause it carries, tag-policy count included", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "undecidable",
                  why: [
                    {
                      kind: "envelope_construct",
                      construct: { kind: "tag_policies", block: { kind: "policies", count: 3 } },
                    },
                    { kind: "super_users_unknown" },
                    {
                      kind: "policy_construct",
                      policy_id: 5,
                      policy_name: "ops-time-boxed",
                      construct: { kind: "validity_schedules" },
                    },
                  ],
                },
                attribution: null,
              },
            ],
          },
        ],
        tags: [],
      } satisfies ExplainResponse,
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    expect(within(result).getByTestId("decision-badge")).toHaveTextContent("Undecidable");

    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(3);

    // The number is a number, in a real sentence.
    expect(causes[0]).toHaveTextContent(/3 tag policies/);
    expect(causes[0]).not.toHaveTextContent(/TagPolicies|Unmodelled|\{/);
    expect(causes[1]).toHaveTextContent(/super-user/i);
    expect(causes[2]).toHaveTextContent("ops-time-boxed");
    expect(causes[2]).toHaveTextContent(/validity schedule/i);
  });

  /**
   * Item 4. A policy egret's compiler does not own still decides real
   * requests — operator-written raw JSON, the delegator plumbing, Ranger's own
   * superuser short-circuit. Dropping it would recreate, inside the new
   * mechanism, the blindness this package removes. There is no governance page
   * for it, so there is no link.
   */
  test("a policy with no model attribution renders its Ranger name and id, unlinked", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            accesses: [
              {
                access_type: "select",
                decision: { kind: "allowed", policy_id: 404, policy_name: "ops-written-select" },
                attribution: null,
              },
            ],
          },
        ],
        tags: [],
      } satisfies ExplainResponse,
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    const raw = within(result).getByTestId("raw-policy");
    expect(raw).toHaveAttribute("data-kind", "unowned");
    expect(raw).toHaveTextContent("ops-written-select");
    expect(raw).toHaveTextContent("404");
    expect(raw).toHaveTextContent(/outside the permission model/i);
    expect(within(raw).queryByRole("link")).not.toBeInTheDocument();
  });

  /**
   * Item 5. Auto-bind seeds thirteen `egret-admin` hierarchies granting `*` on
   * everything onto every generated profile. For an administrator this is the
   * EXPECTED explanation — the old page could not see it at all and answered
   * "no grant matched" while the engine allowed.
   */
  test("a seeded egret-admin policy reads as the expected explanation, not an alarm", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "allowed",
                  policy_id: 2,
                  policy_name: "egret-admin - catalog, schema, table, column",
                },
                attribution: null,
              },
            ],
          },
        ],
        tags: [],
      } satisfies ExplainResponse,
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    const raw = within(result).getByTestId("raw-policy");
    expect(raw).toHaveAttribute("data-kind", "seeded-admin");
    expect(raw).toHaveTextContent("egret-admin - catalog, schema, table, column");
    expect(raw).toHaveTextContent(/expected/i);
    expect(raw).toHaveTextContent(/every generated profile/i);
    // Reassuring, not an alarm: it is not dressed as a misconfiguration.
    expect(raw).not.toHaveTextContent(/unexpected|misconfigured/i);
    expect(within(result).getByTestId("decision-badge")).toHaveTextContent("Allowed");
  });

  /**
   * Item 6 — the pre-existing D10 defect. Before this task a clearance denial
   * hit no entry in the reason map at all, so it rendered the raw wire string
   * with "Unrecognized reason class." and dropped both the gating tag and the
   * clearing roles: the two facts a steward most needs. `required_roles` is
   * OR across roles — holding any one clears the gate.
   */
  test("a clearance decision renders a real label, its tag, and the clearing roles", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "denied",
                  policy_id: 31,
                  policy_name: "perm-clearance-33333333-3333-4333-8333-333333333333",
                  reason: "deny_policy_item",
                },
                attribution: {
                  kind: "clearance",
                  tag_id: "33333333-3333-4333-8333-333333333333",
                  tag: "pii",
                  required_roles: ["pii_cleared", "auditor"],
                  locator: null,
                },
              },
            ],
          },
        ],
        tags: ["pii"],
      } satisfies ExplainResponse,
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    const clearance = within(result).getByTestId("clearance-attribution");
    expect(clearance).toHaveTextContent(/clearance required/i);
    expect(clearance).not.toHaveTextContent(/Unrecognized reason class/);
    expect(clearance).toHaveTextContent("pii");
    expect(clearance).toHaveTextContent("pii_cleared");
    expect(clearance).toHaveTextContent("auditor");
    // OR across roles, stated: any ONE of them clears the gate.
    expect(clearance).toHaveTextContent(/any one/i);
  });

  /**
   * Item 7. The dissolved paragraph. `explain` now evaluates the downloaded
   * artifact, so it DOES see hand-written Ranger policy; a limit that no longer
   * holds is worse than no limit, because it teaches a reader to discount the
   * ones that still do. What replaces the groups paragraph is what is true now:
   * a role- or group-granted policy matches nobody on these engines, so
   * `explain` sends neither.
   */
  test("the honest limits no longer claim explain cannot see hand-written policy", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.queryByTestId("honest-limits")).not.toBeInTheDocument();

    await submit(user);
    const result = await screen.findByTestId("explain-result");
    const limits = within(result).getByTestId("honest-limits");

    expect(limits).not.toHaveTextContent(/cannot see hand-written Ranger policy/i);
    expect(limits).not.toHaveTextContent(/seeded admin policy.*not allowed/i);
    // And the replacement, which is true: neither roles nor groups are sent,
    // because no engine egret provisions can match either.
    expect(limits).toHaveTextContent(/no roles download/i);
    expect(limits).toHaveTextContent(/usersync/i);
    expect(limits).toHaveTextContent(/sends neither/i);
    // The tag limit still holds and stays.
    expect(limits).toHaveTextContent(/names both a schema and a table/i);
  });

  test("tags render honestly when the target carries none", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { outcome: "no_profile_serves_the_catalog", profiles: [], tags: [] },
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    expect(result).toHaveTextContent("None.");
  });

  test("a 403 renders as a forbidden message, not a bare error", async () => {
    mutateAsync.mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    expect(await screen.findByText(/policy:read/)).toBeInTheDocument();
  });
});

describe("ExplainAccess — a disabled Explain button says what it is waiting for", () => {
  /**
   * SP4b-2 finding 2, the button half. `Explain` starts disabled and, with
   * `policy:read` alone, can never enable — the target picker has no options
   * (it names the missing `catalog_registry:read` itself; see
   * `ScopePicker.test.tsx`). Either way the button must not sit there inert
   * with nothing said.
   */
  test("names both missing inputs, then stops explaining once they are supplied", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.getByRole("button", { name: /Explain/ })).toBeDisabled();
    expect(screen.getByText("Explain needs a user and a target catalog.")).toBeInTheDocument();

    await user.type(screen.getByLabelText("User"), "alice-doe");
    expect(screen.getByText("Explain needs a target catalog.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "set-scope" }));
    expect(screen.getByRole("button", { name: /Explain/ })).toBeEnabled();
    expect(screen.queryByText(/^Explain needs/)).not.toBeInTheDocument();
  });
});

describe("ExplainAccess — the subject is a user and nothing else", () => {
  /**
   * **SP4d-2a Task 5 removed `groups` from `ExplainSubject`** — the third field
   * removed from that struct for the third version of one reason: a
   * caller-supplied value cannot stand in for a fact only egret can verify.
   * `roles` and `groups` reach the evaluator EMPTY because egret serves no
   * roles download and runs no usersync, so a policy item naming either
   * matches nobody on any engine egret provisions. A field whose value can
   * change no answer lies about what the endpoint does, so the input is gone
   * and the request carries neither key.
   */
  test("offers no Groups or Roles field and sends neither key", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.queryByLabelText("Groups")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Roles")).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText(/add a group/i)).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText(/add a role/i)).not.toBeInTheDocument();

    await submit(user);

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    const sent = mutateAsync.mock.calls[0][0].data.subject;
    expect(sent).not.toHaveProperty("groups");
    expect(sent).not.toHaveProperty("roles");
    expect(sent.user).toBe("alice-doe");
  });
});

describe("ExplainAccess — the subject's ref is the allocated username (fact 5/5)", () => {
  /**
   * **THE DISCRIMINATING CASE (review M-2).** The engine sees a bare
   * `username`; `list_principals` returns an email as well, and a page that
   * stored the email would get "no policy matched" — a wrong answer that looks
   * exactly like a real one, on the page built to explain why. The suggestion
   * shows `stored as …` so the operator can see which value is kept, and the
   * explain request carries that value.
   */
  test("selecting a suggestion sends the username, not the email", async () => {
    vi.mocked(policyAdminListPrincipals).mockResolvedValue({
      status: 200,
      data: [
        {
          kind: "user",
          name: "alice@egret.local",
          label: "Alice Doe",
          username: "alice-doe-2",
        },
      ],
      headers: new Headers(),
    } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);

    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByLabelText("User"));

    const option = await screen.findByRole("button", { name: /Alice Doe/ });
    expect(option).toHaveTextContent("stored as alice-doe-2");
    await user.click(option);

    await user.click(screen.getByRole("button", { name: "set-scope" }));
    await user.click(screen.getByRole("button", { name: /Explain/ }));

    expect(mutateAsync).toHaveBeenCalled();
    expect(mutateAsync.mock.calls[0][0].data.subject.user).toBe("alice-doe-2");
  });

  /** A principal with no allocated username has no ref the engine could ever
   *  see, so it is shown and NOT selectable — never an invented slug (the
   *  collision suffix `-2` above is unreproducible client-side). */
  test("a principal with no allocated username is not offered", async () => {
    vi.mocked(policyAdminListPrincipals).mockResolvedValue({
      status: 200,
      data: [{ kind: "user", name: "bob@egret.local", label: "Bob Roe", username: null }],
      headers: new Headers(),
    } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);

    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByLabelText("User"));

    expect(await screen.findByText(/no allocated username yet/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Bob Roe/ })).not.toBeInTheDocument();
  });
});
