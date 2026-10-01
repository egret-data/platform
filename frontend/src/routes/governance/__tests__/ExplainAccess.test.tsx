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
          // **`""`, because that is what the server sends.** `eval.rs:759-763`
          // returns `policy_name: String::new()` and `policy_id: None` for this
          // reason by construction — there IS no deciding policy, that is what
          // the verdict means. This fixture used to say `"unnamed"`, a value
          // `explain` cannot produce, and that invention is the whole reason the
          // browser gate found Finding 1 and these tests did not.
          decision: { kind: "denied", policy_id: null, policy_name: "", reason: "no_policy_matched" },
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

  /**
   * **Browser-gate Finding 1 (Major), measured 2026-09-29.** `no_policy_matched`
   * means NOTHING matched, so `egret-ranger-eval` sends `policy_name: ""` and
   * `policy_id: None` by construction. The page used to route every
   * non-`Undecidable` decision through the raw-policy block, so it printed
   * "Outside the permission model … because it decided this access. / Ranger
   * policy , id unknown" — two untruths and a hole where the name goes, and it
   * sends a steward into raw Ranger hunting for a policy that does not exist.
   *
   * The same CLASS as D10's `clearance_required` defect: a decision variant
   * falling into a branch written for a different variant.
   */
  test("no_policy_matched names no deciding policy, because there is none", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 12,
            service_name: "egret-starrocks-prod",
            engine: "starrocks",
            accesses: [
              {
                access_type: "select",
                // Exactly the payload the browser gate captured.
                decision: {
                  kind: "denied",
                  policy_id: null,
                  policy_name: "",
                  reason: "no_policy_matched",
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
    const row = within(result).getByTestId("explain-access");
    expect(within(row).getByTestId("decision-badge")).toHaveTextContent("No policy matched");

    // No decider block at all, and none of its claims.
    expect(within(result).queryByTestId("raw-policy")).not.toBeInTheDocument();
    expect(row).not.toHaveTextContent(/outside the permission model/i);
    expect(row).not.toHaveTextContent(/because it decided this access/i);
    expect(row).not.toHaveTextContent(/Ranger policy/);
    expect(row).not.toHaveTextContent(/id unknown/);
    // And it says so, so nobody goes looking in raw Ranger.
    expect(row).toHaveTextContent(/no deciding policy to open/i);
  });

  /**
   * **Browser-gate Finding 2 (Major), forced live by deleting the `tags` row
   * while the compiled clearance policy survived** — a state this page's own
   * copy says occurs. The payload was `tag: null, required_roles: []`, and the
   * page rendered "Gating tag unknown — the tag row is gone" beside "No role
   * clears this gate: it declares only groups": two sentences that contradict
   * each other, the second FALSE (the gate declared `['sp4d2a_cleared']`), and
   * the clearing role — the fact D10 exists to surface — silently absent rather
   * than reported as unknown.
   *
   * The fix was on the wire, not in the guard: `required_roles` is an
   * `Option`, and `attribution_for` in `crates/egret-perm-compile/src/routes.rs`
   * sends `null` on the one path that cannot look a gate up. So the payload
   * here is `required_roles: null`, and the page no longer has to infer
   * "unknown" from a second field.
   */
  test("an unresolved clearance reports its roles as unknown, not as groups-only", async () => {
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
                  policy_id: 30,
                  policy_name:
                    "perm-clearance-88888888-8888-4888-8888-888888888888-sp4d2a_wh.sales.orders.ssn",
                  reason: "deny_policy_item",
                },
                attribution: {
                  kind: "clearance",
                  tag_id: "88888888-8888-4888-8888-888888888888",
                  tag: null,
                  required_roles: null,
                  locator: "sp4d2a_wh.sales.orders.ssn",
                },
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
    const clearance = within(result).getByTestId("clearance-attribution");
    expect(clearance).toHaveTextContent(/the tag row is gone/i);
    // The false half must be gone: nothing is known about this gate.
    expect(clearance).not.toHaveTextContent(/declares only groups/i);
    // And the missing fact is reported as unknown rather than omitted.
    expect(clearance).toHaveTextContent(/clearing roles are unknown/i);
  });

  /** Finding 2's other half — the case where the empty list IS readable, and the
   *  honest 3→2 this task settles for. `Some([])` is still two facts: a gate
   *  that declares only `required_groups`, and a gate row already deleted
   *  (`clearance_gates_for_tags_via` filters on
   *  `cardinality(required_roles) > 0`, so a missing row and a groups-only row
   *  both miss the lookup). What it can no longer mean is "nothing was looked
   *  up" — that is `null` now. Both branches are pinned so the two cannot be
   *  confused again. */
  test("a resolved clearance with no roles states that, without claiming it is unknown", async () => {
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
                  policy_id: 30,
                  policy_name: "perm-clearance-88888888-8888-4888-8888-888888888888",
                  reason: "deny_policy_item",
                },
                attribution: {
                  kind: "clearance",
                  tag_id: "88888888-8888-4888-8888-888888888888",
                  tag: "sp4d2a_ssn",
                  required_roles: [],
                  locator: null,
                },
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
    const clearance = within(result).getByTestId("clearance-attribution");
    expect(clearance).toHaveTextContent("sp4d2a_ssn");
    expect(clearance).toHaveTextContent(/no clearing role is recorded/i);
    expect(clearance).not.toHaveTextContent(/clearing roles are unknown/i);
    expect(clearance).not.toHaveTextContent(/the tag row is gone/i);
  });

  /**
   * **THE PAYLOAD BELOW CANNOT ARISE FROM THE LIVE PATH TODAY — this test is
   * evidence about the page, not about production.** `attribution_for`
   * (`crates/egret-perm-compile/src/routes.rs`) is the only producer of this
   * variant and it sends `required_roles: null` only when `tag` is `null`, so
   * `tag` resolved + roles `null` is a shape nothing emits.
   *
   * It is here because it is the ONLY fixture that discriminates: the point of
   * making the field an `Option` is that the roles sentence reads
   * `required_roles` and NOTHING ELSE, and under the old `tag === null` guard
   * this payload rendered the groups-only sentence about a gate nothing had
   * looked up. A page that goes back to consulting `tag` fails here and nowhere
   * else. If the wire ever gains a path that emits this shape, this comment is
   * what needs deleting — not the test.
   */
  test("a clearance whose roles are null is unknown even when the tag resolved (shape no live path emits)", async () => {
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
                  policy_id: 30,
                  policy_name: "perm-clearance-88888888-8888-4888-8888-888888888888",
                  reason: "deny_policy_item",
                },
                attribution: {
                  kind: "clearance",
                  tag_id: "88888888-8888-4888-8888-888888888888",
                  tag: "sp4d2a_ssn",
                  required_roles: null,
                  locator: null,
                },
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
    const clearance = within(result).getByTestId("clearance-attribution");
    // The tag IS known, and still nothing is claimed about the gate's roles.
    expect(clearance).toHaveTextContent("sp4d2a_ssn");
    expect(clearance).not.toHaveTextContent(/the tag row is gone/i);
    expect(clearance).toHaveTextContent(/clearing roles are unknown/i);
    expect(clearance).not.toHaveTextContent(/no clearing role is recorded/i);
    expect(clearance).not.toHaveTextContent(/declares only groups/i);
  });

  /**
   * **The blast radius, which the gate measured and asked to be stated rather
   * than changed.** `explain` passes `resolved_tags: None` unconditionally, so a
   * single tag-addressed rule or clearance anywhere turns EVERY verdict on every
   * catalog the profile serves into `Undecidable` — in the gate's fixture, all
   * seven Trino answers. The refusal is sound (the alternative fails open); the
   * scale is what a steward needs to know, so it is said where they read it, and
   * only on the cause it is true of.
   */
  test("a tag-policy refusal says its scope, and other causes do not borrow it", async () => {
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
                      construct: { kind: "tag_policies", block: { kind: "policies", count: 2 } },
                    },
                    { kind: "super_users_unknown" },
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
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes[0]).toHaveTextContent(/2 tag policies/);
    expect(causes[0]).toHaveTextContent(/every catalog this profile serves/i);
    expect(causes[0]).toHaveTextContent(/installation/i);
    // Content-derived, not stapled onto every undecidable cause.
    expect(causes[1]).not.toHaveTextContent(/every catalog this profile serves/i);
  });

  /**
   * **The count is content-derived, and a count of ONE is the case the suite
   * did not hold** (whole-branch review, the surviving mutant). Every other
   * fixture here carries 2 or 3 tag policies, so replacing
   * `count === 1 ? "policy" : "policies"` with a bare `"policies"` left all
   * nineteen tests green. The browser gate falsified it live — deleting one of
   * two tag policies moved the sentence to "1 tag policy" — but nothing in the
   * suite could have caught the regression, and a count that reads "1 tag
   * policies" is the shape a hard-coded number wears.
   */
  test("one tag policy is named in the singular", async () => {
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
                      construct: { kind: "tag_policies", block: { kind: "policies", count: 1 } },
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
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes[0]).toHaveTextContent(/\b1 tag policy\b/);
    expect(causes[0]).not.toHaveTextContent(/1 tag policies/);
  });

  /**
   * **SP4d-2b Task 3, fix round 1 (review Minor 4).** Task 2 and Task 3 added
   * four `WireUndecidableCause` variants and a four-armed
   * `WireTagResourceProblem`, and the only thing holding them was `tsc`'s
   * `const _never: never` guard on `causeSentence`. That guard proves the switch
   * is EXHAUSTIVE. It proves nothing about what any arm renders — an arm
   * returning the wrong sentence, the wrong field, or `""` is exhaustive and
   * wrong, and would have shipped.
   *
   * One fixture carrying all four causes plus all four problem shapes, asserted
   * per cause: each must name the thing a steward would act on (the policy, the
   * value, the shape), and none may render empty.
   */
  test("the tag causes each render a sentence naming what a steward must act on", async () => {
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
                    { kind: "tags_unresolved" },
                    {
                      kind: "tag_resource_not_reproducible",
                      policy_id: 8839,
                      policy_name: "clearance-a",
                      problem: { kind: "not_only_the_tag_resource" },
                    },
                    {
                      kind: "tag_resource_not_reproducible",
                      policy_id: 8840,
                      policy_name: "clearance-b",
                      problem: { kind: "excludes" },
                    },
                    {
                      kind: "tag_resource_not_reproducible",
                      policy_id: 8841,
                      policy_name: "clearance-c",
                      problem: { kind: "value_not_trie_prefix_exact", value: "p*x" },
                    },
                    {
                      kind: "tag_resource_not_reproducible",
                      policy_id: 8842,
                      policy_name: "clearance-d",
                      problem: { kind: "recursive" },
                    },
                    {
                      kind: "tag_sub_policy_is_deny_capable",
                      policy_id: 8799,
                      policy_name: "mask-pii",
                    },
                    {
                      kind: "resource_candidacy_not_reproducible",
                      policy_id: 77,
                      policy_name: "excludes-orders",
                    },
                    {
                      kind: "tag_sub_result_split_by_resource_policy",
                      tag_policy_id: 8799,
                      tag_policy_name: "mask-pii",
                      resource_policy_id: 42,
                      resource_policy_name: "mask-ssn",
                    },
                  ],
                },
                attribution: null,
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
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(8);

    // Every arm renders SOMETHING — the failure an exhaustive switch cannot
    // catch is an arm that returns "".
    for (const cause of causes) {
      expect(cause.textContent?.trim().length ?? 0).toBeGreaterThan(20);
    }

    // `tags_unresolved`: the distinction it exists to make is "did not resolve"
    // versus "there are none", so the sentence has to carry it.
    expect(causes[0]).toHaveTextContent(/did not resolve/i);
    expect(causes[0]).toHaveTextContent(/not the same as there being none/i);

    // The four `tag_resource_not_reproducible` shapes have four different
    // remedies, so each must read differently and name its own policy.
    expect(causes[1]).toHaveTextContent("clearance-a");
    expect(causes[1]).toHaveTextContent(/resource level other than/i);
    expect(causes[2]).toHaveTextContent("clearance-b");
    expect(causes[2]).toHaveTextContent(/isExcludes/);
    expect(causes[3]).toHaveTextContent("clearance-c");
    expect(causes[3]).toHaveTextContent(/p\*x/);
    expect(causes[4]).toHaveTextContent("clearance-d");
    expect(causes[4]).toHaveTextContent(/isRecursive/);
    const problemSentences = [causes[1], causes[2], causes[3], causes[4]].map(
      (c) => c.textContent,
    );
    expect(new Set(problemSentences).size).toBe(4);

    expect(causes[5]).toHaveTextContent("mask-pii");
    expect(causes[5]).toHaveTextContent(/deny items/i);

    // The candidate-set refusal has to name the policy AND say that the filter
    // is what egret cannot reproduce — "undecidable" alone sends a steward
    // looking at the policy's ITEMS, which are not the problem.
    expect(causes[6]).toHaveTextContent("excludes-orders");
    expect(causes[6]).toHaveTextContent(/trie|filter/i);

    // The split names BOTH policies — a sentence naming only one of them is the
    // shape that sends a steward to the wrong policy, which is the whole reason
    // this cause refuses instead of answering.
    expect(causes[7]).toHaveTextContent("mask-pii");
    expect(causes[7]).toHaveTextContent("mask-ssn");
    expect(causes[7]).toHaveTextContent(/higher-priority/i);
  });

  /**
   * **SP4d-2b Task 5.** The differential gate found that the TAG policy
   * repository — and no other — normalises its policies on the way in, which
   * added `tag_policy_not_normalisable` with three problem shapes. `tsc` proves
   * the switch reaches them; only this proves each says something different.
   *
   * The third shape is the one that must NOT read like the other two. A
   * namespaced access or mask type is something a steward edits; an absent mask
   * type makes the engine's plugin fail to load the whole download, so nothing
   * it serves is being decided. A sentence that said "egret cannot decide this
   * access" for that would understate an incident as a gap.
   */
  test("the tag normalisation cause separates an editable name from a plugin that will not load", async () => {
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
                      kind: "tag_policy_not_normalisable",
                      policy_id: 8901,
                      policy_name: "clearance-ns-access",
                      problem: { kind: "namespaced_access_type", access_type: "trino:select" },
                    },
                    {
                      kind: "tag_policy_not_normalisable",
                      policy_id: 8902,
                      policy_name: "mask-ns-type",
                      problem: { kind: "namespaced_mask_type", mask_type: "hive:MASK_HASH" },
                    },
                    {
                      kind: "tag_policy_not_normalisable",
                      policy_id: 8903,
                      policy_name: "mask-no-type",
                      problem: { kind: "mask_type_absent" },
                    },
                    {
                      kind: "tag_priority_arbitration_not_modelled",
                      policy_ids: [8910, 8911],
                      policy_names: ["clearance-low", "clearance-high"],
                      priorities: [0, 3],
                    },
                    {
                      kind: "tag_deny_reopened_differs_by_plugin_version",
                      plugin_version: "ranger_2_5_0",
                      tag_policy_id: 8920,
                      tag_policy_name: "clearance-deny",
                      resource_policy_id: 55,
                      resource_policy_name: "grant-orders",
                    },
                    {
                      kind: "tag_deny_reopened_differs_by_plugin_version",
                      plugin_version: null,
                      tag_policy_id: 8921,
                      tag_policy_name: "clearance-deny-unknown",
                      resource_policy_id: 56,
                      resource_policy_name: "grant-invoices",
                    },
                  ],
                },
                attribution: null,
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
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(6);
    for (const cause of causes) {
      expect(cause.textContent?.trim().length ?? 0).toBeGreaterThan(20);
    }

    // Each names its own policy and quotes its own offending value, or a steward
    // cannot find the thing to change.
    expect(causes[0]).toHaveTextContent("clearance-ns-access");
    expect(causes[0]).toHaveTextContent("trino:select");
    expect(causes[1]).toHaveTextContent("mask-ns-type");
    expect(causes[1]).toHaveTextContent("hive:MASK_HASH");
    expect(causes[2]).toHaveTextContent("mask-no-type");

    // …and the third says the PLUGIN is failing, not that egret is unsure. This
    // is the assertion that would fail if someone folded all three onto one
    // sentence.
    expect(causes[2]).toHaveTextContent(/fail|not load|loading/i);
    expect(causes[2]).not.toHaveTextContent(/component-qualified/i);

    // The priority arbitration has to name BOTH policies AND both priorities:
    // the priorities are the thing a steward changes, and a sentence that named
    // only the policies would leave them with nothing to do.
    expect(causes[3]).toHaveTextContent("clearance-low");
    expect(causes[3]).toHaveTextContent("clearance-high");
    expect(causes[3]).toHaveTextContent(/priority 0/);
    expect(causes[3]).toHaveTextContent(/priority 3/);

    // ⚠ FIX ROUND 1, IMPORTANT 4. The version split renders DIFFERENTLY depending
    // on whether the plugin version is known, and the two sentences give different
    // remedies. The old single sentence told the steward to record the plugin
    // version — which `explain` now always does, so in the only case `explain` can
    // produce it the advice had already been taken and could not be taken again.
    //
    // Known 2.5.0: names the version, says which side egret does not reproduce,
    // and must NOT tell anyone to record anything.
    expect(causes[4]).toHaveTextContent("clearance-deny");
    expect(causes[4]).toHaveTextContent("grant-orders");
    expect(causes[4]).toHaveTextContent(/2\.5\.0/);
    expect(causes[4]).toHaveTextContent(/2\.8\.0/);
    expect(causes[4]).not.toHaveTextContent(/records which plugin version/i);

    // Unknown: this is the ONE case where "record the plugin version" is the
    // remedy, and it must still be offered.
    expect(causes[5]).toHaveTextContent("clearance-deny-unknown");
    expect(causes[5]).toHaveTextContent("grant-invoices");
    expect(causes[5]).toHaveTextContent(/records which plugin version/i);

    expect(new Set(causes.map((c) => c.textContent)).size).toBe(6);
  });

  /**
   * **SP4d-2b Task 4.** `tsc`'s `const _never: never` proves the switch covers
   * the new `tag_attachment_not_reproducible` cause and its three problem
   * shapes; it proves nothing about what any of them renders. All three shapes
   * here, asserted apart: each must name the TAG (the construct is on the tag
   * download, not on a policy, so naming a policy would send a steward to the
   * wrong place), each must carry its own payload, and no two may read alike.
   */
  test("the tag attachment cause renders all three shapes, each naming the tag", async () => {
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
                      kind: "tag_attachment_not_reproducible",
                      tag: "pii",
                      problem: { kind: "options", keys: ["TAG_VALIDITY_PERIODS"] },
                    },
                    {
                      kind: "tag_attachment_not_reproducible",
                      tag: "phi",
                      problem: { kind: "validity_periods", count: 2 },
                    },
                    {
                      kind: "tag_attachment_not_reproducible",
                      tag: "pci",
                      problem: { kind: "resource_levels", levels: ["catalog", "schema"] },
                    },
                    // The other two `resource_levels` lengths, which the
                    // `BTreeSet` change introduced branches for. Both are LIVE:
                    // the cause fires only for fewer than three levels, so 1 and
                    // 0 are the whole remaining domain — and 1 is the shape the
                    // Rust side went out of its way to pin.
                    {
                      kind: "tag_attachment_not_reproducible",
                      tag: "ssn",
                      problem: { kind: "resource_levels", levels: ["catalog"] },
                    },
                    {
                      kind: "tag_attachment_not_reproducible",
                      tag: "iban",
                      problem: { kind: "resource_levels", levels: [] },
                    },
                  ],
                },
                attribution: null,
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
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(5);

    for (const cause of causes) {
      expect(cause.textContent?.trim().length ?? 0).toBeGreaterThan(20);
    }

    expect(causes[0]).toHaveTextContent("pii");
    expect(causes[0]).toHaveTextContent("TAG_VALIDITY_PERIODS");
    expect(causes[1]).toHaveTextContent("phi");
    expect(causes[1]).toHaveTextContent(/2 validity periods/);
    expect(causes[2]).toHaveTextContent("pci");
    expect(causes[2]).toHaveTextContent("catalog, schema");
    // Plural for two levels, SINGULAR for one, and a phrase rather than an empty
    // gap for none. `tsc` proves the switch is exhaustive and nothing about what
    // any arm renders — a branch that reads "at the resource levels catalog" or
    // "at the resource levels " is exhaustive and wrong.
    expect(causes[2]).toHaveTextContent("resource levels catalog, schema");
    expect(causes[3]).toHaveTextContent("ssn");
    expect(causes[3]).toHaveTextContent("resource level catalog");
    expect(causes[3]).not.toHaveTextContent("resource levels");
    expect(causes[4]).toHaveTextContent("iban");
    expect(causes[4]).toHaveTextContent(/none at all/);

    // Five shapes, five sentences. Two that read alike would hide which remedy
    // applies.
    expect(new Set(causes.map((c) => c.textContent)).size).toBe(5);
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
   * roles download and runs no usersync, so a policy item naming a role, or a
   * group OTHER THAN `public`, matches nobody on any engine egret provisions.
   * A field whose value can change no answer lies about what the endpoint
   * does, so the input is gone and the request carries neither key.
   *
   * **`public` is the exception, and the reason removing the field loses
   * nothing.** `egret-ranger-eval` matches `public` by NAME, independently of
   * what the request carries, because Ranger does — so a policy granting to
   * `public` decides here either way, and a caller could never have changed
   * that by naming it. Every other group name is a value only egret could
   * verify and egret cannot.
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
