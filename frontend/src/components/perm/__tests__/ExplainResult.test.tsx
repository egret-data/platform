import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi, type Mock } from "vitest";

/**
 * How an explain answer reads (`ExplainResultView`) and the subject's user
 * field (`SubjectUserField`), rendered directly. These tests lived in
 * `ExplainAccess.test.tsx` and reached the components through the Explain
 * access page's form; the page is gone (DG redesign #2, P8). Its form-level
 * tests (the disabled Explain button, a user-only subject, a refused question)
 * are Check access tests now (`routes/governance/people/__tests__/CheckAccess.test.tsx`).
 *
 * **SP4d-2a Task 8.** `explain` evaluates the policy set each engine actually
 * downloads and answers once per PROFILE, three-valued. So this file is about
 * that shape — two profiles are two results, "no profile serves this catalog"
 * is its own state and not a denial, an `Undecidable` names every cause it
 * carries, and a policy egret's compiler does not own is reported rather than
 * dropped.
 *
 * `answer` stands for the server: each test queues the response
 * `POST /perm/explain` would send (`{ status, data }`), and `renderAnswer`
 * renders its `data` as Check access does.
 */
vi.mock("@/api/generated/policy-admin/policy-admin");

import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { ExplainResponse } from "@/api/generated/model";
import { ExplainResultView, SubjectUserField } from "@/components/perm/ExplainResult";

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
      tags: [{ name: "pii", attached_at_column: true }],
      tag_policies_in_download: 1,
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
      tags: [{ name: "pii", attached_at_column: true }],
      tag_policies_in_download: 1,
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
};

let answer: Mock<() => Promise<{ status: number; data: ExplainResponse }>>;

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter>
      <QueryClientProvider client={qc}>{ui}</QueryClientProvider>
    </MemoryRouter>,
  );
}

/** Renders the next queued server answer, as Check access renders a 200. */
async function renderAnswer() {
  const res = await answer();
  return wrap(<ExplainResultView result={res.data} />);
}

beforeEach(() => {
  vi.clearAllMocks();
  answer = vi.fn().mockResolvedValue({ status: 200, data: TWO_PROFILES });
  vi.mocked(policyAdminListPrincipals).mockResolvedValue({
    status: 200,
    data: [],
    headers: new Headers(),
  } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);
});

describe("ExplainResultView — one answer per profile, over the artifact the engine downloads", () => {
  /** Item 1. Two profiles are two results, each naming itself and its engine. */
  test("two profiles render as two results, each naming its profile and engine", async () => {
    await renderAnswer();

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
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    expect(result).not.toHaveTextContent(/only a summary/i);
    expect(result).not.toHaveTextContent("Not allowed");
    // One decision badge per access, and not one more.
    expect(screen.getAllByTestId("decision-badge")).toHaveLength(3);
  });

  /** Item 1's deep links, which now live on the per-profile rows. */
  test("a grant attribution links to that grant, and a rule attribution to that rule", async () => {
    await renderAnswer();

    await screen.findByTestId("explain-result");

    expect(screen.getByRole("link", { name: "View grant grant-1" })).toHaveAttribute(
      "href",
      "/governance/grants/grant-1",
    );
    const ruleLink = screen.getByRole("link", { name: "View rule rule-1" });
    expect(ruleLink).toHaveAttribute("href", "/governance/rules/rule-1");
    // A per-column StarRocks rule expands ONE row into N policies; the locator
    // is the only thing that says which expansion decided.
    expect(screen.getByTestId("rule-locator")).toHaveTextContent("sales.public.orders.ssn");
  });

  /** Item 2. Nothing enforcing is a different fact from something refusing. */
  test("a catalog no profile serves is its own state, not a denial", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: { outcome: "no_profile_serves_the_catalog", profiles: [] },
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const state = within(result).getByTestId("no-profile-state");
    expect(state).toHaveTextContent(/nothing is enforcing/i);
    // C12: the state's own sentence is the short one ("this is not a refusal");
    // the long "not a denial" paragraph moved, unchanged, under the folded
    // internals of the limits block. Both still say it is not a refusal.
    expect(state).toHaveTextContent(/not a refusal/i);
    expect(within(result).getByTestId("honest-limits")).toHaveTextContent(/not a denial/i);

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    expect(within(result).getByTestId("decision-badge")).toHaveTextContent("Undecidable");

    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(3);

    // The number is a number, in a real sentence.
    expect(causes[0]).toHaveTextContent(/3 tag policies/);
    expect(causes[0]).not.toHaveTextContent(/TagPolicies|Unmodelled|\{/);
    // ⚠ The WORDING, asserted since fix round 1 (Minor 2). SP4d-2b Task 3 made the
    // evaluator evaluate the block, so "which egret does not evaluate yet" became a
    // promise about a shipped feature; Task 7 swept it and nothing pinned the
    // replacement, so a mutation restoring the superseded clause survived.
    //
    // **This arm is currently UNREACHABLE from the backend** — both envelope
    // sweeps (`request_preamble` and `tag_candidates`) `continue` on
    // `Unmodelled::TagPolicies(TagPolicyBlock::Policies(_))`, and `PolicyConstruct`
    // carries only per-policy constructs — so this fixture is a payload egret does
    // not produce today. The arm exists to keep `explain_wire.rs` exhaustive
    // without a wildcard, and the assertion exists so its words cannot rot
    // unnoticed; it is NOT evidence that a steward can reach this sentence.
    expect(causes[0]).toHaveTextContent(/in a block that was not evaluated/i);
    expect(causes[0]).not.toHaveTextContent(/does not evaluate yet/i);
    expect(causes[1]).toHaveTextContent(/super-user/i);
    expect(causes[2]).toHaveTextContent("ops-time-boxed");
    expect(causes[2]).toHaveTextContent(/validity schedule/i);
  });

  test("a plugin-version cause names where the version came from for either kind of engine", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 0,
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "undecidable",
                  why: [{ kind: "plugin_version_mismatch", engine: "trino", declared: "ranger_2_9_0" }],
                },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const cause = (await screen.findAllByTestId("undecidable-cause"))[0];
    expect(cause).toHaveTextContent("Ranger 2.9.0");
    // A provisioned engine's version comes from its image, not from a
    // declaration on the profile; an external one's from its operator.
    expect(cause).not.toHaveTextContent(/This profile declares/);
    expect(cause).toHaveTextContent(/image/);
    expect(cause).toHaveTextContent(/declared/);
  });

  /**
   * Item 4. A policy egret's compiler does not own still decides real
   * requests — operator-written raw JSON, the delegator plumbing, Ranger's own
   * superuser short-circuit. Dropping it would recreate, inside the new
   * mechanism, the blindness this package removes. There is no governance page
   * for it, so there is no link.
   */
  test("a policy with no model attribution renders its Ranger name and id, unlinked", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: { kind: "allowed", policy_id: 404, policy_name: "ops-written-select" },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    await renderAnswer();
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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 12,
            service_name: "egret-starrocks-prod",
            engine: "starrocks",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
   * **The blast radius, REWRITTEN by SP4d-2b Task 7 rather than deleted.**
   *
   * SP4d-2a's gate measured that `explain` passed `resolved_tags: None`
   * unconditionally, so a single tag-addressed rule or clearance anywhere turned
   * every verdict on every catalog a profile serves into `Undecidable` — all
   * seven Trino answers in that fixture. egret resolves the tags now and that
   * state is gone, so the note may no longer claim an installation-wide reach it
   * does not have: what is left is a property of this PROFILE's download.
   *
   * The sibling below pins the other surviving shape, `tags_unresolved`, whose
   * reach is a CATALOG and whose remedy is a stored assignment. The two must not
   * read alike — one note covering both would tell a steward to go looking in the
   * wrong place — so the last assertion here compares them.
   */
  test("a tag-download refusal says its scope, and other causes do not borrow it", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "undecidable",
                  why: [
                    {
                      // ⚠ `unreadable`, not `policies` (fix round 1, Low 4): the
                      // `policies` arm is unreachable from the backend, and the
                      // note this test is about must be pinned on a payload egret
                      // can actually produce. The count and its pluralisation are
                      // pinned on the unreachable arm elsewhere, where they belong.
                      kind: "envelope_construct",
                      construct: { kind: "tag_policies", block: { kind: "unreadable" } },
                    },
                    { kind: "super_users_unknown" },
                  ],
                },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes[0]).toHaveTextContent(/could not read/i);
    // ⚠ Asserted on the SCOPE element, not through the <li>: the sentence and the
    // note are two claims and the <li> holds both, so an assertion through it is
    // satisfied by whichever of the two still carries the words (SP4d-2b Task 7 —
    // a mutation that emptied the sentence survived a suite that did this).
    const scope0 = within(causes[0]).getByTestId("undecidable-cause-scope");
    expect(scope0).toHaveTextContent(/every catalog this profile serves/i);
    expect(scope0).toHaveTextContent(/tag-policy download this profile received/i);
    // ⚠ The claim SP4d-2a's note made and Task 7 removed. A tag policy in a
    // profile's block no longer costs every verdict in the installation an
    // answer, so a note still saying so would be a measured statement that has
    // become false — the defect class this package counts as equal to a
    // superseded branch.
    expect(causes[0]).not.toHaveTextContent(/installation/i);
    // Content-derived, not stapled onto every undecidable cause.
    expect(causes[1]).not.toHaveTextContent(/every catalog this profile serves/i);
    expect(causes[1]).not.toHaveTextContent(/not specific to your target/i);
  });

  /**
   * **Fix round 3: every decline state names its own remedy and its own reach —
   * and that is the defect this replaces, not an addition.**
   *
   * Task 7 wrote one sentence for this cause when `EmptyLevel` was the only state
   * that produced it, and rounds 1 and 2 added three more states and left the
   * sentence alone. Measured: a catalog holding `('sales','*','')` was told "a tag
   * assignment … stored with no schema or no table … re-store that assignment on a
   * table or a column" — a row that does not exist and a remedy already satisfied —
   * and a target carrying `Σ` got the same sentence PLUS a scope note claiming
   * "every target in this catalog answers the same way", which is false, because
   * that state is per REQUEST. The old version of this test pinned the wrong
   * wording, so the suite required it.
   *
   * ⚠ **This covered FOUR states until SP4d debt Task 3.** `stored_level_empty` and
   * `stored_wildcard` are gone from the wire enum: migration 0177 adds a `CHECK` to
   * `catalog_tag_assignments` that makes an empty schema or table, and a `*` or `?`
   * in any level, unstorable, so no backend can report them. The two that remain are
   * the two whose reaches differ, which is what this test is really for.
   *
   * `tsc` proves the switch exhaustive and proves nothing about two arms rendering
   * distinguishable sentences, so the distinctness is asserted directly.
   */
  test("each decline state names its own remedy and its own reach", async () => {
    const declines = [
      {
        declined: {
          kind: "stored_fold_divergent",
          schema: "sales",
          table: "tab\u0131e",
          column: "",
        },
        sentence: [/stored at sales \/ tab\u0131e/i, /upper- and lower-case forms/i, /rename that resource/i],
        perCatalog: true,
      },
      {
        declined: { kind: "target_fold_divergent", level: "schema", value: "\u03a3ales" },
        sentence: [/the schema you asked about/i, /\u03a3ales/, /nothing is stored wrongly/i],
        perCatalog: false,
      },
    ] as const;

    const rendered: string[] = [];
    for (const row of declines) {
      answer.mockResolvedValueOnce({
        status: 200,
        data: {
          outcome: "per_profile",
          profiles: [
            {
              service_id: 11,
              service_name: "egret-trino-prod",
              engine: "trino",
              tags: [],
              tag_policies_in_download: 1,
              accesses: [
                {
                  access_type: "select",
                  decision: { kind: "undecidable", why: [{ kind: "tags_unresolved" }] },
                  attribution: null,
                },
              ],
            },
          ],
          tag_resolution_declined: row.declined,
        } satisfies ExplainResponse,
      });
      const view = await renderAnswer();

      const result = await screen.findByTestId("explain-result");
      const cause = within(result).getAllByTestId("undecidable-cause")[0];
      const sentence = within(cause).getByTestId("undecidable-cause-sentence");
      const scope = within(cause).getByTestId("undecidable-cause-scope");

      // The cause's own distinction, true of BOTH states and therefore in the head.
      expect(sentence).toHaveTextContent(/declined to resolve/i);
      expect(sentence).toHaveTextContent(/not the same as there being none/i);
      // …and this state's own subject and remedy.
      for (const pattern of row.sentence) {
        expect(sentence).toHaveTextContent(pattern);
      }
      // ⚠ Round 1's single sentence, which the suite used to require: it must not
      // appear on ANY state now. It belonged to `stored_level_empty`, which migration
      // 0177 made unstorable, so nothing here may still render it.
      expect(sentence).not.toHaveTextContent(/stored with no schema or no table/i);
      expect(sentence).not.toHaveTextContent(/re-store that assignment on a table or a column/i);
      // The REACH, which differs, and whose round-1 wording was measured false for
      // the request-side state.
      if (row.perCatalog) {
        expect(scope).toHaveTextContent(/every target in this catalog answers the same way/i);
        expect(scope).not.toHaveTextContent(/specific to what you asked/i);
      } else {
        expect(scope).toHaveTextContent(/specific to what you asked/i);
        expect(scope).toHaveTextContent(/every other target in this catalog answers normally/i);
        expect(scope).not.toHaveTextContent(/every target in this catalog answers the same way/i);
      }
      rendered.push(sentence.textContent ?? "");
      view.unmount();
    }

    // ⚠ **The literal, not `declines.length`** — fix round, Minor 3. Written against
    // the array's own length this assertion is self-referential: deleting a decline
    // row shrinks both sides and the test stays green, which is the one thing it
    // exists to prevent. `2` is the count the wire enum has, so dropping a state
    // from `ExplainTagDecline` without revisiting this file fails here.
    expect(new Set(rendered).size).toBe(2);
  });

  /**
   * The cause with NO reason attached — a payload egret does not produce, because
   * `tag_resolution_declined` is `Some` exactly when the handler passed
   * `resolved_tags: None`, which is the only production state raising this cause.
   * It is asserted anyway, and what it must NOT do is invent a reach: round 1's
   * note claimed a per-catalog scope unconditionally, and one of the two states
   * is per-request, so with no reason the honest answer is no note at all.
   */
  test("an unresolved-tags cause with no reason renders the head and claims no reach", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "undecidable",
                  why: [
                    { kind: "tags_unresolved" },
                    {
                      kind: "envelope_construct",
                      construct: { kind: "tag_policies", block: { kind: "unreadable" } },
                    },
                  ],
                },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes).toHaveLength(2);

    const sentence = within(causes[0]).getByTestId("undecidable-cause-sentence");
    expect(sentence).toHaveTextContent(/declined to resolve/i);
    expect(sentence).toHaveTextContent(/not the same as there being none/i);
    // No reason, so no remedy may be invented — this is where round 1's sentence
    // named a stored row.
    expect(sentence).not.toHaveTextContent(/no schema or no table/i);
    expect(sentence).not.toHaveTextContent(/re-store/i);
    // And no scope note at all, because the reach is unknown without the reason.
    expect(
      within(causes[0]).queryByTestId("undecidable-cause-scope"),
    ).not.toBeInTheDocument();

    // The sibling cause still carries its own note, so "no note" above is
    // content-derived rather than the element having gone missing everywhere.
    const scope1 = within(causes[1]).getByTestId("undecidable-cause-scope");
    expect(scope1).toHaveTextContent(/tag-policy download this profile received/i);
    expect(sentence.textContent).not.toEqual(scope1.textContent);
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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const causes = within(result).getAllByTestId("undecidable-cause");
    expect(causes[0]).toHaveTextContent(/\b1 tag policy\b/);
    expect(causes[0]).not.toHaveTextContent(/1 tag policies/);
  });

  /**
   * **The composite refusal the concurrent Critical added, split out of the
   * tag-priority sentence.** It first rode `tag_priority_arbitration_not_modelled`
   * as a documented alias, which made that sentence's opening noun — "Several TAG
   * policies" — false for this shape, where one of the two is a RESOURCE policy.
   *
   * `tsc` proves the switch is exhaustive and says NOTHING about what a branch
   * renders, so this is the assertion that the new sentence names which policy is
   * which, carries both priorities (the thing a steward changes), and does not imply
   * the state is common — measured at 0 of 1148 composed-random cases and 10 of 4000
   * overall.
   */
  test("the composite priority refusal names which policy is the tag one", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "undecidable",
                  why: [
                    {
                      kind: "access_priority_composite_not_modelled",
                      tag_policy_id: 8799,
                      tag_policy_name: "perm-rule-pii-mask",
                      tag_priority: 0,
                      resource_policy_id: 320,
                      resource_policy_name: "perm-grant-analyst",
                      resource_priority: 7,
                    },
                  ],
                },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const sentence = within(result).getByTestId("undecidable-cause-sentence");
    // Which is which — the half the positional lists could not say.
    expect(sentence).toHaveTextContent(/the tag policy “perm-rule-pii-mask”/);
    expect(sentence).toHaveTextContent(/the resource policy “perm-grant-analyst”/);
    // Both priorities, because equalising them is the remedy.
    expect(sentence).toHaveTextContent(/at priority 0/);
    expect(sentence).toHaveTextContent(/higher priority 7/);
    expect(sentence).toHaveTextContent(/same priority makes this decidable/i);
    // And it must not read as a widespread misconfiguration.
    expect(sentence).toHaveTextContent(/rarely/i);
    // ⚠ It must NOT borrow the tag-only sentence, which is the defect it was split
    // out of: that one calls both policies tag policies.
    expect(sentence).not.toHaveTextContent(/several tag policies/i);
  });

  /**
   * ⚠ **L3 (whole-branch review): two of the five causes below are states
   * `explain` CANNOT produce.** `tag_sub_policy_is_deny_capable` and
   * `tag_sub_result_split_by_resource_policy` are raised only from the evaluator's
   * `decide_sub` / `evaluate_sub` — the mask and row-filter entry points — and
   * `explain` calls neither, so this fixture asserts a sentence no live response
   * can carry. It is kept, not deleted: `explain_wire.rs` mirrors every variant by
   * build break, and these are the sentences the page will need the day `explain`
   * reports a mask verdict. Until then they are coverage of the MIRROR, not of a
   * reachable state — the concrete form of this branch's scope finding.
   */
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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    // Task 7 changed this sentence's MEANING, so the assertion moved with it:
    // the cause is no longer "egret never resolves tags" but "egret declined,
    // for this catalog, because of a stored assignment". The distinction it has
    // always existed to make — declining is not finding none — is still asserted,
    // because an empty set would have been an ANSWER.
    // This fixture attaches no `tag_resolution_declined`, so the head is all this
    // cause can say — the remedy moved into that field in fix round 3, and the
    // state-specific wording is asserted by its own test above.
    const unresolved = within(causes[0]).getByTestId("undecidable-cause-sentence");
    expect(unresolved).toHaveTextContent(/declined to resolve/i);
    expect(unresolved).toHaveTextContent(/not the same as there being none/i);

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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
    // ⚠ "tag policies" is correct HERE, and the sibling test below is why: the
    // composite shape — where one of the two is a RESOURCE policy — has its own
    // cause and its own sentence, so this one is raised only where both are tag
    // policies. It briefly said "Several policies" while the two shared a wire tag.
    expect(causes[3]).toHaveTextContent(/several tag policies at different priorities/i);

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
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 1,
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
      } satisfies ExplainResponse,
    });
    await renderAnswer();

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

  /**
   * **Task 8's browser gate, F4 — measured on a real Trino, twice.** The tag list
   * was `tag_keys_for_resource_via`'s exact-match query while every verdict beside
   * it came from the rolled-up resolution, so a request DENIED by a tag policy
   * through a tag attached to the TABLE above the column rendered the deciding
   * policy's name directly above "None.", and a target reported `Allowed` with
   * "None." was one no query could read at all.
   *
   * The field is the evaluator's own set now, and the assertion that matters is
   * `attached_at_column: false` reading as "on the table" — the half that says
   * which row to change.
   */
  test("a tag that reaches this column from the table says so, beside the deny it caused", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [
          { name: "ancdeny", attached_at_column: false },
          { name: "pii", attached_at_column: true },
        ],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: {
                  kind: "denied",
                  policy_id: 60,
                  policy_name: "tp-clearance-ancdeny",
                  reason: "deny_policy_item",
                },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    // ⚠ **Scoped to the badge, review M1.** These used to be asserted against the
    // whole `explain-result`, where the limits paragraph already contains "a tag
    // marked on the table reaches this column" — so BOTH regexes matched whatever
    // the ternary did, and swapping `attached_at_column ? … : …` was 31/31 green.
    // Inverted, that label sends a steward to edit the wrong
    // `catalog_tag_assignments` row, which is the one thing the field exists to
    // prevent. Per badge, the pairing is pinned.
    const tags = within(result).getAllByTestId("profile-tag");
    expect(tags).toHaveLength(2);
    expect(tags[0]).toHaveTextContent(/^ancdeny\s*on the table$/);
    expect(tags[1]).toHaveTextContent(/^pii\s*on this column$/);
    // And the old heading's claim is gone: this list IS what the verdict used.
    expect(result).not.toHaveTextContent(/decide nothing here/i);
    expect(result).not.toHaveTextContent(/not the set the engine would resolve/i);
  });

  /**
   * **Task 8's fix round — the open question, ruled on.** The heading said "Tags the
   * engine resolves for this resource", which reads as a complete account of tag
   * governance on the target and is not one: the list is the set for the REQUEST,
   * and a tag attached to a column does not appear when the target is its table,
   * because the engine's enricher admits SELF and ANCESTOR attachments and never
   * DESCENDANT ones. Measured in the live fixture — `sp4d1.t2` with no column
   * answers `[]` while `sp4d1.t2.item` answers `pii` — and the engine agrees, since
   * the live gate measured a tag-addressed row filter NOT firing at table
   * granularity for a column-level tag.
   *
   * The ruling was to fix the CLAIM and not the data: no second query, no second
   * list, because one list derived from the evaluator's own input is the property
   * F4/F5 were fixed to get. So the label names the request and one sentence names
   * the asymmetry — and both are asserted here, because `tsc` proves neither and
   * the heading is exactly the kind of sentence that gets widened again.
   */
  test("the tag list is labelled as the request's, and says what it does not reach", async () => {
    await renderAnswer();
    const result = await screen.findByTestId("explain-result");

    // The label: the REQUEST, not "this resource" and not anything that reads as
    // every tag on the target.
    // ⚠ Narrowed again by review H1/H2/H3: the heading is per PROFILE now, and
    // "this request" was still one level wider than the data — a request can be
    // answered by two profiles of which only one consults tags.
    expect(result).toHaveTextContent(/Tags that could decide here/i);
    expect(result).not.toHaveTextContent(/Tags that decide this request/i);
    expect(result).not.toHaveTextContent(/Tags the engine resolves for this resource/i);
    expect(result).not.toHaveTextContent(/Tags egret has recorded on this resource/i);

    // And the asymmetry, in words, with the remedy attached.
    const limits = within(result).getByTestId("honest-limits");
    expect(limits).toHaveTextContent(/reaches up the hierarchy and never down/i);
    expect(limits).toHaveTextContent(/ask about the column to see a tag stored on it/i);
    // Auditing a table is not auditing its columns — the sentence a steward needs
    // BEFORE they conclude a table carries nothing.
    expect(limits).toHaveTextContent(/auditing a table is therefore not the same/i);
  });

  /**
   * **F4's second half.** A decline is not "none": `tag_resolution_declined` means
   * egret did not resolve at all, and an empty list rendered as "None." there tells
   * a steward nothing is attached when nobody looked. The reason is already beside
   * the verdict; this block must not contradict it.
   */
  test("a declined resolution is not reported as the target carrying no tags", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 1,
            accesses: [
              {
                access_type: "select",
                decision: { kind: "undecidable", why: [{ kind: "tags_unresolved" }] },
                attribution: null,
              },
            ],
          },
        ],
        tag_resolution_declined: {
          kind: "stored_fold_divergent",
          schema: "sales",
          table: "orders",
          column: "ema\u0131l",
        },
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    expect(result).toHaveTextContent(/not resolved/i);
    expect(result).toHaveTextContent(/not the same as there being\s+none/i);
    // ⚠ **Review M2: the guard this replaces could not fail.**
    // `not.toHaveTextContent(/^None\.$/m)` never matched, because jest-dom
    // normalises whitespace across the whole element — rendering "None." BESIDE
    // the decline note was 31/31 green, so the test's stated subject was
    // unguarded. The three branches now have their own test ids, and the
    // assertion is the ABSENCE OF THE ELEMENT, which a mutation cannot satisfy by
    // adding text somewhere else.
    expect(within(result).queryByTestId("profile-tags-none")).not.toBeInTheDocument();
    expect(within(result).queryByTestId("profile-tag")).not.toBeInTheDocument();
    expect(within(result).getByTestId("profile-tags-declined")).toBeInTheDocument();
  });

  /**
   * **Task 8's browser gate, F3.** The limits block rendered "mask and row-filter
   * verdicts arrive with SP4d-2b" — on the SP4d-2b branch, where they do not
   * arrive. No test could see it, because a test asserts the string and not whether
   * the string is true. This one asserts the shape of the claim instead: the limit
   * stands, and it is dated to NOTHING. A version number in this block is the
   * defect, whatever number it is.
   */
  test("the access-only limit states itself and promises no release", async () => {
    await renderAnswer();
    const result = await screen.findByTestId("explain-result");
    const limits = within(result).getByTestId("honest-limits");

    // Task 12, M-5: the limit names what IS shown (the StarRocks mask), so it
    // no longer contradicts the mask line above it.
    expect(limits).toHaveTextContent(/answers about access only/i);
    expect(limits).toHaveTextContent(/On a StarRocks profile, a read of a column also shows the mask/i);
    expect(limits).not.toHaveTextContent(/ACCESS only\. A column the engine masks/);
    // ⚠ No dated promise, in either direction: not this package, not a successor.
    expect(limits).not.toHaveTextContent(/arrive with/i);
    expect(limits).not.toHaveTextContent(/SP4d/i);
    expect(limits).not.toHaveTextContent(/\bsp\d/i);
    // And what replaces it is the consequence a steward can act on.
    expect(limits).toHaveTextContent(/can still return masked values/i);
  });

  /**
   * **⚠ Whole-branch review H3, and this test used to be the correlated oracle.**
   * It asserted `"None."` for `no_profile_serves_the_catalog` with an EMPTY tag
   * list — the one case where the defect is invisible. The defect was that the
   * response carried a tag set at all when nothing was evaluated: with a sibling
   * catalog's assignment on the same warehouse (`catalog_tag_assignments` has no
   * catalog column), the resolution returns `pii`, and the page rendered "Nothing
   * is enforcing" and "Tags that decide this request — pii" on one screen, with no
   * verdicts in existence. A reviewer's probe asserting exactly that payload
   * passed 32/32.
   *
   * It is fixed at the LEVEL rather than in the renderer: the tag set lives on
   * `ExplainProfile`, so a response with no profiles has nowhere to carry one and
   * the probe's payload no longer type-checks. This test therefore pins the thing
   * that was wrong — that nothing tag-related renders for this outcome — rather
   * than the empty case that could never have caught it.
   */
  test("a catalog nothing serves renders no tag block at all", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: { outcome: "no_profile_serves_the_catalog", profiles: [] },
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    expect(within(result).getByTestId("no-profile-state")).toBeInTheDocument();
    // No heading, no badge, no "none" — there are no verdicts for a tag to decide,
    // so there is nothing honest to say about tags here.
    expect(result).not.toHaveTextContent(/tags that could decide/i);
    expect(within(result).queryByTestId("profile-tag")).not.toBeInTheDocument();
    expect(within(result).queryByTestId("profile-tags-none")).not.toBeInTheDocument();
    expect(within(result).queryByTestId("profile-tags-declined")).not.toBeInTheDocument();
    expect(within(result).queryByTestId("profile-tags-no-policies")).not.toBeInTheDocument();
    // ⚠ And neither do the LIMITS paragraphs that talk about per-profile tag lists
    // — found in the browser after the structural fix, the same class one notch
    // further down: prose about "each profile's tag list" and "those sets" under
    // "Nothing is enforcing", where there are no profiles and no lists. The limits
    // that still hold stay (they are about the page, not about the answer).
    const limits = within(result).getByTestId("honest-limits");
    expect(limits).not.toHaveTextContent(/each profile’s tag list/i);
    expect(limits).not.toHaveTextContent(/those sets are for the resource/i);
    expect(limits).toHaveTextContent(/answers about access only/i);
  });

  /**
   * **Whole-branch review H1.** A catalog served by two profiles — one whose
   * download carries tag policies and one whose does not — used to render ONE tag
   * list below both, so the page said these tags decide this request for a profile
   * where nothing tag-related decided anything. The resolution is the same set for
   * both (it is a property of the target); what differs is whether the profile
   * could consult it, and that is now carried per profile.
   *
   * Both halves are asserted in ONE render, because the defect was exactly that
   * the two profiles shared a claim: the first must say so, the second must not.
   */
  test("a profile whose download carries no tag policies says so instead of listing tags", async () => {
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "with-tag-service",
            engine: "trino",
            tags: [{ name: "pii", attached_at_column: true }],
            tag_policies_in_download: 2,
            accesses: [
              {
                access_type: "select",
                decision: { kind: "allowed", policy_id: 7, policy_name: "grant-a" },
                attribution: null,
              },
            ],
          },
          {
            service_id: 12,
            service_name: "no-tag-service",
            engine: "starrocks",
            tags: [],
            tag_policies_in_download: 0,
            accesses: [
              {
                access_type: "select",
                decision: { kind: "allowed", policy_id: 8, policy_name: "grant-b" },
                attribution: null,
              },
            ],
          },
        ],
      } satisfies ExplainResponse,
    });
    await renderAnswer();

    const result = await screen.findByTestId("explain-result");
    const blocks = within(result).getAllByTestId("explain-profile");
    expect(blocks).toHaveLength(2);

    // The profile that CAN consult tags lists them.
    expect(within(blocks[0]).getAllByTestId("profile-tag")).toHaveLength(1);
    expect(within(blocks[0]).queryByTestId("profile-tags-no-policies")).not.toBeInTheDocument();

    // The profile that cannot says why, and lists nothing — the claim that was
    // false before this field existed.
    expect(within(blocks[1]).queryByTestId("profile-tag")).not.toBeInTheDocument();
    expect(within(blocks[1]).getByTestId("profile-tags-no-policies")).toHaveTextContent(
      /carries no tag policies, so no tag decided anything here/i,
    );
  });
});

/**
 * View governance Task 10, revised in Task 12. A StarRocks read explain of a
 * column carries the masked-read verdict (`mask`). StarRocks checks no access on
 * a column a mask transforms (live finding C2), so then there is no `select`
 * verdict and `masked_read_skips_access` is set. The `create materialized view`
 * row is gone (M-4, N2): the page states the engine-wide rule instead.
 */
describe("ExplainResultView — the StarRocks mask and the materialized-view rule", () => {
  const UNCLEARED_MASKED: ExplainResponse = {
    outcome: "per_profile",
    profiles: [
      {
        service_id: 12,
        service_name: "egret-starrocks-prod",
        engine: "starrocks",
        tags: [],
        tag_policies_in_download: 0,
        accesses: [],
        masked_read_skips_access: true,
        mask: {
          access_type: "select",
          decision: {
            kind: "masked",
            result: {
              policy_id: 1,
              policy_name: "perm-clearance-t1-mask-sales.public.orders.ssn",
              mask_type: "MASK_NULL",
              masked_value: null,
              mask_condition: null,
            },
          },
          attribution: null,
        },
      },
    ],
  };

  test("an uncleared read of a masked StarRocks column says NULL via the mask, not denied", async () => {
    answer.mockResolvedValue({ status: 200, data: UNCLEARED_MASKED });
    await renderAnswer();

    const profile = await screen.findByTestId("explain-profile");
    const mask = within(profile).getByTestId("explain-mask");
    expect(mask).toHaveTextContent(/returns NULL/i);
    expect(mask).toHaveTextContent("MASK_NULL");
    expect(mask).toHaveTextContent("perm-clearance-t1-mask-sales.public.orders.ssn");
    const skipped = within(profile).getByTestId("explain-access-not-checked");
    expect(skipped).toHaveTextContent(/not access-checked on StarRocks/i);
    // Final review V-1: keyed on the mask type, whoever denies the column, and
    // the query-level consequence explain cannot evaluate is stated.
    expect(skipped).toHaveTextContent(/its mask is MASK_NULL/i);
    expect(skipped).toHaveTextContent(/whoever denies it/i);
    expect(skipped).toHaveTextContent(/unless the reader may select another column it reads/i);
    expect(skipped).toHaveTextContent(/first unmasked column \(measured: id, on a partly classified/i);
    expect(skipped).toHaveTextContent(/when no column is unmasked, on an Iceberg table/i);
    expect(skipped).toHaveTextContent(/hidden _file \/ _pos columns/i);
    expect(skipped).toHaveTextContent("___count___");
    expect(skipped).not.toHaveTextContent(/a reader who holds the grant/i);
    // No verdict badge at all: nothing says "denied" for this read.
    expect(within(profile).queryByTestId("decision-badge")).not.toBeInTheDocument();
    expect(profile).not.toHaveTextContent(/No access type was evaluated/i);
  });

  test("a cleared reader's MASK_NONE is not called masked, and no materialized-view verdict is shown", async () => {
    const cleared: ExplainResponse = {
      outcome: "per_profile",
      profiles: [
        {
          ...UNCLEARED_MASKED.profiles[0],
          masked_read_skips_access: undefined,
          accesses: [
            {
              access_type: "select",
              decision: { kind: "allowed", policy_id: 4, policy_name: "perm-grant-g1" },
              attribution: null,
            },
          ],
          mask: {
            access_type: "select",
            decision: {
              kind: "masked",
              result: {
                policy_id: 1,
                policy_name: "perm-clearance-t1-mask-sales.public.orders.ssn",
                mask_type: "MASK_NONE",
                masked_value: null,
                mask_condition: null,
              },
            },
            attribution: null,
          },
        },
      ],
    };
    answer.mockResolvedValue({ status: 200, data: cleared });
    await renderAnswer();

    const profile = await screen.findByTestId("explain-profile");
    const mask = within(profile).getByTestId("explain-mask");
    expect(mask).not.toHaveTextContent(/masked to/i);
    expect(mask).not.toHaveTextContent(/returns NULL/i);
    expect(mask).toHaveTextContent(/not transformed/i);
    expect(within(profile).queryByTestId("explain-access-not-checked")).not.toBeInTheDocument();
    // N2: no red "denied" materialized-view row for a cleared reader; the rule instead.
    expect(profile).not.toHaveTextContent(/default_catalog\./);
    const rows = within(profile).getAllByTestId("explain-access");
    expect(rows.some((r) => r.textContent?.includes("create materialized view"))).toBe(false);
    expect(within(profile).getByTestId("explain-mv-rule")).toHaveTextContent(
      /in every database to anyone not cleared for every classification/i,
    );
  });

  // Fix round 1 (review m-5): with an undecidable mask the access verdict is
  // kept, and the page says it is provisional.
  test("an undecidable StarRocks mask marks the access verdict provisional", async () => {
    const undecidable: ExplainResponse = {
      outcome: "per_profile",
      profiles: [
        {
          ...UNCLEARED_MASKED.profiles[0],
          masked_read_skips_access: undefined,
          accesses: [
            {
              access_type: "select",
              decision: {
                kind: "denied",
                policy_id: 2,
                policy_name: "perm-clearance-t1-sales.public.orders.ssn",
                reason: "deny_policy_item",
              },
              attribution: null,
            },
          ],
          mask: {
            access_type: "select",
            decision: { kind: "undecidable", why: [{ kind: "tags_unresolved" }] },
            attribution: null,
          },
        },
      ],
    } as unknown as ExplainResponse;
    answer.mockResolvedValue({ status: 200, data: undecidable });
    await renderAnswer();

    const profile = await screen.findByTestId("explain-profile");
    const note = within(profile).getByTestId("explain-mask-provisional");
    expect(note).toHaveTextContent(/provisional/i);
    expect(note).toHaveTextContent(/if StarRocks applies a MASK_NULL mask here/i);
  });

  // Final review V-1 / Task 10 M4: once any policy's MASK_NULL can decide, the
  // steward needs to see which one, so the mask line carries its attribution.
  test("the mask line names the perm object that decided the mask", async () => {
    const attributed: ExplainResponse = {
      outcome: "per_profile",
      profiles: [
        {
          ...UNCLEARED_MASKED.profiles[0],
          mask: {
            ...UNCLEARED_MASKED.profiles[0].mask!,
            attribution: {
              kind: "clearance",
              tag_id: "33333333-3333-4333-8333-333333333333",
              tag: "pii",
              required_roles: ["pii_cleared"],
              locator: "mask-sales.public.orders.ssn",
            },
          },
        },
      ],
    };
    answer.mockResolvedValue({ status: 200, data: attributed });
    await renderAnswer();
    const profile = await screen.findByTestId("explain-profile");
    const by = within(profile).getByTestId("explain-mask-decided-by");
    expect(within(by).getByTestId("clearance-attribution")).toHaveTextContent("pii");
    expect(by).toHaveTextContent("pii_cleared");
  });

  test("a hand-written mask is named as outside the permission model", async () => {
    const hand: ExplainResponse = {
      outcome: "per_profile",
      profiles: [
        {
          ...UNCLEARED_MASKED.profiles[0],
          mask: {
            access_type: "select",
            decision: {
              kind: "masked",
              result: {
                policy_id: 31,
                policy_name: "hand mask",
                mask_type: "MASK_NULL",
                masked_value: null,
                mask_condition: null,
              },
            },
            attribution: null,
          },
        },
      ],
    };
    answer.mockResolvedValue({ status: 200, data: hand });
    await renderAnswer();
    const profile = await screen.findByTestId("explain-profile");
    const by = within(profile).getByTestId("explain-mask-decided-by");
    expect(by).toHaveTextContent(/did not mint this policy/i);
    expect(by).toHaveTextContent("hand mask");
  });

  test("an undecidable or absent mask names no deciding policy", async () => {
    const none: ExplainResponse = {
      outcome: "per_profile",
      profiles: [
        {
          ...UNCLEARED_MASKED.profiles[0],
          masked_read_skips_access: undefined,
          mask: { access_type: "select", decision: { kind: "no_mask" }, attribution: null },
        },
      ],
    } as unknown as ExplainResponse;
    answer.mockResolvedValue({ status: 200, data: none });
    await renderAnswer();
    const profile = await screen.findByTestId("explain-profile");
    expect(within(profile).getByTestId("explain-mask")).toHaveTextContent(/no mask applies/i);
    expect(within(profile).queryByTestId("explain-mask-decided-by")).not.toBeInTheDocument();
  });

  test("a decided mask carries no provisional note", async () => {
    answer.mockResolvedValue({ status: 200, data: UNCLEARED_MASKED });
    await renderAnswer();
    const profile = await screen.findByTestId("explain-profile");
    expect(within(profile).queryByTestId("explain-mask-provisional")).not.toBeInTheDocument();
  });

  test("a profile with no mask verdict shows no mask line", async () => {
    await renderAnswer();

    await screen.findByTestId("explain-result");
    expect(screen.queryByTestId("explain-mask")).not.toBeInTheDocument();
  });
});

describe("SubjectUserField — the subject's ref is the allocated username (fact 5/5)", () => {
  /**
   * **THE DISCRIMINATING CASE (review M-2).** The engine sees a bare
   * `username`; `list_principals` returns an email as well, and a field that
   * stored the email would get "no policy matched" — a wrong answer that looks
   * exactly like a real one, on the surface built to explain why. The
   * suggestion shows `stored as …` so the operator can see which value is
   * kept, and the field hands that value on.
   */
  test("selecting a suggestion stores the username, not the email", async () => {
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
    const onChange = vi.fn();

    const user = userEvent.setup();
    wrap(<SubjectUserField value="" onChange={onChange} />);
    await user.click(screen.getByLabelText("User"));

    const option = await screen.findByRole("option", { name: /Alice Doe/ });
    expect(option).toHaveTextContent("stored as alice-doe-2");
    await user.click(option);

    expect(onChange).toHaveBeenLastCalledWith("alice-doe-2");
    expect(onChange).not.toHaveBeenCalledWith("alice@egret.local");
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
    wrap(<SubjectUserField value="" onChange={vi.fn()} />);
    await user.click(screen.getByLabelText("User"));

    expect(await screen.findByText(/no allocated username yet/)).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Bob Roe/ })).not.toBeInTheDocument();
  });
});

describe("ExplainResultView — an engine grant (F11)", () => {
  /**
   * A decision attributed to `perm-engine-{uuid}` names the engine grant and the
   * Ranger policy, and says nothing about a clearance: before this branch the
   * attribution fell through to the clearance block, which assumes a tag.
   */
  test("renders the engine grant and its Ranger policy, and no clearance", async () => {
    const id = "44444444-4444-4444-8444-444444444444";
    answer.mockResolvedValueOnce({
      status: 200,
      data: {
        outcome: "per_profile",
        profiles: [
          {
            service_id: 11,
            service_name: "egret-trino-prod",
            engine: "trino",
            tags: [],
            tag_policies_in_download: 0,
            accesses: [
              {
                access_type: "select",
                decision: { kind: "allowed", policy_id: 70, policy_name: `perm-engine-${id}` },
                attribution: { kind: "engine_grant", engine_grant_id: id },
              },
            ],
          },
        ],
      },
    });
    await renderAnswer();
    expect(screen.getByText(`Engine grant ${id}`)).toBeInTheDocument();
    expect(screen.getByText(`perm-engine-${id}`)).toBeInTheDocument();
    expect(screen.queryByTestId("clearance-attribution")).toBeNull();
  });
});

/**
 * Classification UX C12: the verdict comes first, one visible line keeps the
 * limit that changes what *Allowed* means, and the engine internals fold under
 * a closed disclosure — still in the DOM and one click away, text unchanged.
 */
describe("ExplainResultView — verdict first, internals folded (C12)", () => {
  test("leads with the verdict, keeps one visible limit line, and folds the rest", () => {
    wrap(<ExplainResultView result={TWO_PROFILES} />);
    expect(screen.getByText(/Masks on Trino and row filters are not shown here/)).toBeVisible();
    // The verdicts precede the limits block in document order.
    const result = screen.getByTestId("explain-result");
    const firstProfile = within(result).getAllByTestId("explain-profile")[0];
    const limits = within(result).getByTestId("honest-limits");
    expect(firstProfile.compareDocumentPosition(limits) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const details = screen.getByText("How this answer was worked out").closest("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
    const internals = within(details as HTMLElement).getByText(/egret serves no roles download and runs no usersync/);
    expect(internals).not.toBeVisible();
    expect(within(details as HTMLElement).getByText(/Each plugin polls for its policy set/)).toBeInTheDocument();
  });

  test("opening the disclosure shows the internals", async () => {
    wrap(<ExplainResultView result={TWO_PROFILES} />);
    await userEvent.click(screen.getByText("How this answer was worked out"));
    expect(screen.getByText(/egret serves no roles download and runs no usersync/)).toBeVisible();
  });

  test("says nothing is enforced in one short paragraph, the long one folded", () => {
    wrap(<ExplainResultView result={{ outcome: "no_profile_serves_the_catalog", profiles: [] }} />);
    expect(
      within(screen.getByTestId("no-profile-state")).getByText(
        "No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it.",
      ),
    ).toBeVisible();
    const details = screen.getByText("How this answer was worked out").closest("details") as HTMLElement;
    expect(within(details).getByText(/no policy set egret compiles\s+covers it/)).toBeInTheDocument();
  });
});
