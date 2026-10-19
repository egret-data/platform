import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { ApiError } from "@/api/client";
import type { ClassifiedPolicy, MigrateResponse, UndoResponse } from "@/api/policies";
import {
  classifyServicePolicies,
  migrateServicePolicies,
  undoServicePolicyMigration,
} from "@/api/policies";
import { MigrateTab } from "@/routes/governance/engines/tabs/MigrateTab";
import { E_TR, grantsResponse, ok, openArea, pluginResponse, policy, profileDto, serve } from "./engineHarness";

/**
 * The engine page's Migrate tab (spec §6.4), ported from
 * `routes/governance/__tests__/PolicyMigrationPage.test.tsx` (the operator's
 * page, SP4d-3 Task 9): every case of that file, against the tab, which takes
 * the profile as props. The adapters in `@/api/policies` are
 * mocked; the page, `FilterableTable` and the real primitives render for real.
 *
 * The fixture is deliberately NOT in a convenient order (id 16 before 11) and
 * mixes every kind of row, so "render the first row" or "every row is
 * selectable" cannot pass by accident.
 */

const INFRA_REASON =
  "This policy is managed by the platform (infra): it is what lets the engine's own service account work, so it is not offered for migration.";
const COMPILER_REASON =
  "This policy is compiled by egret's permission model from a grant; edit the grant instead.";
const CATALOG_BLOCKER =
  'The catalog "lake_unregistered" is not in the catalog registry (names are matched exactly, and `*` names no catalog), so this policy cannot be migrated as it stands.';
const MIGRATED_SENTENCE =
  "This policy was migrated into egret's permission model on 2026-10-10T08:00:00Z by admin@egret.local (1 grant, 0 rules); undo the migration to restore it.";
const GENERATED_NOTICE =
  "After this migration every listed profile is compiled by egret's permission model. Its remaining hand-written policies can still be edited and deleted, but a NEW hand-written policy can no longer be created in it — add grants or rules instead.";
const UNDO_NOTICE =
  "Every profile this migration recompiled stays compiled by egret's permission model after the undo: its hand-written policies can be edited and deleted, but a NEW hand-written policy still cannot be created in it.";
const BUDGET_REFUSAL =
  "proving this migration needs 15000 probe evaluations across 2 profile(s), 3100000 units of work (probe evaluations x the policies each visits), over the limit of 1300000. The work grows with the size of each affected profile's download (the largest serves 340 policies), roughly with its square, not with how many policies this request migrates. The probe set is never cut short — a partial proof would be reported as a whole one — so this migration cannot be proved until those profiles are smaller.";

const ROWS: ClassifiedPolicy[] = [
  {
    id: 16,
    name: "perm-grant-0f1e",
    managed_by: "compiler",
    enabled: true,
    migratable: false,
    warnings: [],
    blockers: [COMPILER_REASON],
  },
  {
    id: 11,
    name: "allow-analysts",
    managed_by: "operator",
    enabled: true,
    migratable: true,
    warnings: [],
    blockers: [],
    proposed: { grants: 1, rules: 0 },
  },
  {
    id: 12,
    name: "engine-service-account",
    managed_by: "infra",
    enabled: true,
    migratable: false,
    warnings: [],
    blockers: [INFRA_REASON],
  },
  {
    id: 13,
    name: "unregistered-catalog",
    managed_by: "operator",
    enabled: true,
    migratable: false,
    warnings: [],
    blockers: [CATALOG_BLOCKER],
  },
  {
    id: 14,
    name: "allow-finance",
    managed_by: "operator",
    enabled: true,
    migratable: true,
    warnings: [],
    blockers: [],
    proposed: { grants: 2, rules: 1 },
  },
  {
    id: 15,
    name: "old-sales",
    managed_by: "operator",
    enabled: false,
    migratable: false,
    warnings: [],
    blockers: [MIGRATED_SENTENCE],
    migrated: {
      policy_name: "old-sales",
      grant_ids: ["g-1"],
      rule_ids: [],
      probes_proved: 384,
      migrated_by: "admin@egret.local",
      migrated_at: "2026-10-10T08:00:00Z",
    },
  },
];

const COMPILE = { services: [7, 9], created: 1, updated: 0, deleted: 0, unchanged: 3, skipped: [] };

function proved(overrides: Partial<MigrateResponse> = {}): MigrateResponse {
  return {
    dry_run: true,
    committed: false,
    equivalent: true,
    refusal: null,
    policies: [{ policy_id: 11, policy_name: "allow-analysts", grant_ids: ["g-9"], rule_ids: [] }],
    profiles: [
      {
        service_id: 7,
        service_name: "trino-prod",
        plugin_versions: ["ranger_2_5_0"],
        probes: 96,
        probes_full: 96,
        disagreements: 0,
        removed_policies: ["allow-analysts"],
        added_policies: ["perm-grant-9a"],
      },
      {
        service_id: 9,
        service_name: "trino-analytics",
        plugin_versions: ["ranger_2_8_0"],
        probes: 40,
        probes_full: 40,
        disagreements: 0,
        removed_policies: [],
        added_policies: ["perm-grant-9a"],
      },
    ],
    probe_evaluations: 136,
    disagreements: [],
    compile: COMPILE,
    notices: [GENERATED_NOTICE],
    ...overrides,
  };
}

vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  listProfiles: () =>
    Promise.resolve([
      { id: 9, name: "trino-analytics", type: "trino", policy_version: 1, policy_count: 2, engines: [], origin: "manual" },
      { id: 7, name: "trino-prod", type: "trino", policy_version: 3, policy_count: 6, engines: [], origin: "manual" },
    ]),
  classifyServicePolicies: vi.fn(),
  migrateServicePolicies: vi.fn(),
  undoServicePolicyMigration: vi.fn(),
}));

const ADMIN: Me = {
  user_id: "u1",
  email: "a@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};
const READER: Me = {
  user_id: "u2",
  email: "r@example.com",
  platform_roles: [],
  permissions: ["policy:read"],
};

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.hash}</output>;
}

function renderPage(me: Me = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/governance/engines/e1?tab=migrate"]}>
            <Routes>
              <Route path="/governance/engines/:id" element={<MigrateTab service="trino-prod" serviceId={7} />} />
              <Route path="/governance" element={<Where />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

/** The table row that carries `name` — scoped queries, so a sentence found
 *  elsewhere on the page (the preview) can never satisfy a row assertion. */
async function rowOf(name: string) {
  const cell = await screen.findByText(name);
  const row = cell.closest("tr");
  if (!row) throw new Error(`no row for ${name}`);
  return row as HTMLElement;
}

/** The Migrate button ELEMENT itself — never an ancestor's disabled state. */
function migrateButton() {
  return screen.getByRole("button", { name: "Migrate" }) as HTMLButtonElement;
}

beforeEach(() => {
  vi.mocked(classifyServicePolicies).mockReset().mockResolvedValue(ROWS);
  vi.mocked(migrateServicePolicies).mockReset().mockResolvedValue(proved());
  vi.mocked(undoServicePolicyMigration).mockReset();
});

describe("MigrateTab — what each policy is", () => {
  it("renders infra and compiler rows locked, with their reason visible, and not selectable", async () => {
    renderPage();
    const infra = await rowOf("engine-service-account");
    expect(within(infra).getByText("infra")).toBeInTheDocument();
    expect(within(infra).getByText("Locked")).toBeInTheDocument();
    expect(within(infra).getByText(INFRA_REASON)).toBeInTheDocument();
    const infraBox = within(infra).getByRole("checkbox", { name: "Select engine-service-account" });
    expect(infraBox).toBeDisabled();

    const compiler = await rowOf("perm-grant-0f1e");
    expect(within(compiler).getByText("compiler")).toBeInTheDocument();
    expect(within(compiler).getByText("Locked")).toBeInTheDocument();
    expect(within(compiler).getByText(COMPILER_REASON)).toBeInTheDocument();
    expect(within(compiler).getByRole("checkbox", { name: "Select perm-grant-0f1e" })).toBeDisabled();

    // The plausible wrong one: an operator row that IS migratable stays pickable.
    const ok = await rowOf("allow-analysts");
    expect(within(ok).getByRole("checkbox", { name: "Select allow-analysts" })).toBeEnabled();
    expect(within(ok).getByText("operator")).toBeInTheDocument();

    // Clicking the locked box changes nothing: the preview stays unrequested.
    await userEvent.setup().click(infraBox);
    expect(infraBox).not.toBeChecked();
  });

  it("renders a blocked operator policy's blocker sentence, offending value included", async () => {
    renderPage();
    const row = await rowOf("unregistered-catalog");
    expect(within(row).getByText(CATALOG_BLOCKER)).toBeInTheDocument();
    expect(within(row).getByText(/lake_unregistered/)).toBeInTheDocument();
    expect(within(row).getByText("Blocked")).toBeInTheDocument();
    expect(within(row).getByRole("checkbox", { name: "Select unregistered-catalog" })).toBeDisabled();
  });

  it("marks an already-migrated original as migrated with an Undo action, not as switched off", async () => {
    renderPage();
    const row = await rowOf("old-sales");
    expect(within(row).getByText("Migrated")).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Undo migration of old-sales" })).toBeInTheDocument();
    expect(within(row).getByText(MIGRATED_SENTENCE)).toBeInTheDocument();
    // Not presented as a policy someone merely switched off.
    expect(within(row).queryByText(/switched off/i)).toBeNull();
    expect(within(row).queryByText(/switch it on/i)).toBeNull();
    expect(within(row).getByRole("checkbox", { name: "Select old-sales" })).toBeDisabled();
    // A row that is not migrated offers no undo.
    const other = await rowOf("allow-analysts");
    expect(within(other).queryByRole("button", { name: /undo/i })).toBeNull();
  });
});

describe("MigrateTab — preview before migrate", () => {
  it("keeps Migrate disabled before a preview, enables it after one, and disables it again when the selection changes", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");

    // 1. Nothing selected, no preview: disabled.
    expect(migrateButton()).toBeDisabled();

    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    // 2. Selected but not previewed: still disabled.
    expect(migrateButton()).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Preview migration" }));
    expect(migrateServicePolicies).toHaveBeenCalledWith(7, ["allow-analysts"], true);
    // 3. A proved preview of THIS selection: enabled.
    await waitFor(() => expect(migrateButton()).toBeEnabled());

    // 4. The selection changes: the preview no longer describes it.
    await user.click(screen.getByRole("checkbox", { name: "Select allow-finance" }));
    expect(migrateButton()).toBeDisabled();
    expect(screen.getByText(/selection changed since that preview/i)).toBeInTheDocument();

    // 5. Back to exactly the previewed set: that preview describes it again
    //    (staleness compares the selection, not a "touched" flag).
    await user.click(screen.getByRole("checkbox", { name: "Select allow-finance" }));
    expect(migrateButton()).toBeEnabled();
  });

  it("migrates exactly the previewed selection for real and re-reads the classification", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));
    await waitFor(() => expect(migrateButton()).toBeEnabled());

    vi.mocked(migrateServicePolicies).mockResolvedValueOnce(proved({ dry_run: false, committed: true }));
    const classifyCallsBefore = vi.mocked(classifyServicePolicies).mock.calls.length;
    await user.click(migrateButton());
    expect(migrateServicePolicies).toHaveBeenLastCalledWith(7, ["allow-analysts"], false);
    await waitFor(() =>
      expect(vi.mocked(classifyServicePolicies).mock.calls.length).toBeGreaterThan(classifyCallsBefore),
    );
  });

  it("lists every profile the migration changes, the other ones marked, and says the profile becomes generated", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    const panel = await screen.findByRole("region", { name: "Affected profiles" });
    expect(within(panel).getByText("This migration changes 2 profiles")).toBeInTheDocument();
    const other = within(panel).getByText("trino-analytics").closest("tr") as HTMLElement;
    expect(within(other).getByText("Also changed")).toBeInTheDocument();
    expect(within(other).getByText("Ranger 2.8.0")).toBeInTheDocument();
    const self = within(panel).getByText("trino-prod").closest("tr") as HTMLElement;
    expect(within(self).queryByText("Also changed")).toBeNull();

    expect(screen.getByText(GENERATED_NOTICE)).toBeInTheDocument();
  });

  /** SP4d-4 live D-3: the preview says how much pruning saved, and why it
   *  kept every probe when it did. */
  it("shows the pruned probe count beside the full one, and the reason when pruning fell back", async () => {
    const user = userEvent.setup();
    const base = proved();
    vi.mocked(migrateServicePolicies).mockResolvedValueOnce(
      proved({
        profiles: [
          { ...base.profiles[0], probes: 6210, probes_full: 273240 },
          {
            ...base.profiles[1],
            probes: 40,
            probes_full: 40,
            pruning_fallback: "ContentFreeConstruct",
          },
        ],
      }),
    );
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    const panel = await screen.findByRole("region", { name: "Affected profiles" });
    const pruned = within(panel).getByText("trino-prod").closest("tr") as HTMLElement;
    expect(within(pruned).getByText("6210")).toBeInTheDocument();
    expect(within(pruned).getByText("of 273240 without pruning")).toBeInTheDocument();
    expect(within(pruned).queryByText(/Not pruned/)).toBeNull();

    const full = within(panel).getByText("trino-analytics").closest("tr") as HTMLElement;
    expect(within(full).getByText("of 40 without pruning")).toBeInTheDocument();
    expect(
      within(full).getByText(
        "Not pruned: a policy uses conditions, schedules or delegation the checker cannot decide around",
      ),
    ).toBeInTheDocument();
  });

  /** Final review (Task 10 Minor 1): a preview is rolled back, so the compiled
   *  policy's `perm-grant-<id>` name carries an id that will never exist. The
   *  preview counts it by kind instead of presenting that id as a fact. */
  it("counts a preview's compiled policies by kind instead of naming the rolled-back id", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    const panel = await screen.findByRole("region", { name: "Affected profiles" });
    const self = within(panel).getByText("trino-prod").closest("tr") as HTMLElement;
    expect(
      within(self).getByText("1 compiled grant policy (named when written)"),
    ).toBeInTheDocument();
    expect(within(panel).queryByText(/perm-grant-9a/)).toBeNull();
    // The removed side is a stored policy's real name, and stays named.
    expect(within(self).getByText("allow-analysts")).toBeInTheDocument();
  });

  /** F11: an engine grant compiles to `perm-engine-<id>`, another compiler
   *  prefix; a preview counts it by kind like the other three. */
  it("counts a preview's compiled engine grant policies by kind", async () => {
    const user = userEvent.setup();
    const base = proved();
    vi.mocked(migrateServicePolicies).mockResolvedValueOnce({
      ...base,
      profiles: [
        { ...base.profiles[0], added_policies: ["perm-grant-9a", "perm-engine-7c"] },
        base.profiles[1],
      ],
    });
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    const panel = await screen.findByRole("region", { name: "Affected profiles" });
    const self = within(panel).getByText("trino-prod").closest("tr") as HTMLElement;
    expect(
      within(self).getByText("1 compiled grant policy, 1 compiled engine grant policy (named when written)"),
    ).toBeInTheDocument();
    expect(within(panel).queryByText(/perm-engine-7c/)).toBeNull();
  });

  it("renders a disagreement as the disagreeing probe, and does not enable Migrate", async () => {
    const user = userEvent.setup();
    vi.mocked(migrateServicePolicies).mockResolvedValue(
      proved({
        equivalent: false,
        disagreements: [
          {
            service_id: 9,
            service_name: "trino-analytics",
            plugin_version: "ranger_2_8_0",
            check: "access",
            user: "bob",
            groups: ["contractors"],
            roles: [],
            tags: [],
            tagging: "hypothetical",
            access_type: "select",
            resource: "lake.sales.orders",
            before: "deny",
            after: "allow",
            sentence:
              'On "trino-analytics" (Ranger 2.8.0), bob would gain select on lake.sales.orders: deny before, allow after.',
          },
        ],
      }),
    );
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    const table = await screen.findByRole("table", { name: "Disagreements" });
    const row = within(table).getByText("bob").closest("tr") as HTMLElement;
    expect(within(row).getByText("select")).toBeInTheDocument();
    expect(within(row).getByText("lake.sales.orders")).toBeInTheDocument();
    expect(within(row).getByText("Ranger 2.8.0")).toBeInTheDocument();
    expect(within(row).getByText("trino-analytics")).toBeInTheDocument();
    expect(within(row).getByText("deny")).toBeInTheDocument();
    expect(within(row).getByText("allow")).toBeInTheDocument();
    expect(within(row).getByText("Hypothetical")).toBeInTheDocument();
    expect(
      screen.getByText(/bob would gain select on lake\.sales\.orders: deny before, allow after\./),
    ).toBeInTheDocument();
    expect(screen.queryByText(/migration failed/i)).toBeNull();
    expect(migrateButton()).toBeDisabled();
  });

  it("renders a probe-budget refusal in the server's words, and does not enable Migrate", async () => {
    const user = userEvent.setup();
    vi.mocked(migrateServicePolicies).mockResolvedValue(
      proved({ equivalent: false, refusal: BUDGET_REFUSAL, profiles: [], probe_evaluations: 0 }),
    );
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));

    expect(await screen.findByText(BUDGET_REFUSAL)).toBeInTheDocument();
    expect(migrateButton()).toBeDisabled();
  });

  it("shows a refused preview's server message (409) and keeps Migrate disabled", async () => {
    const user = userEvent.setup();
    vi.mocked(migrateServicePolicies).mockRejectedValue(
      new ApiError(409, 'conflict: rule name "perm-rule-x" is taken by another rule'),
    );
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));
    expect(await screen.findByText('rule name "perm-rule-x" is taken by another rule')).toBeInTheDocument();
    expect(migrateButton()).toBeDisabled();
  });
});

describe("MigrateTab — undo", () => {
  it("undoes after confirmation and says the profile stays generated", async () => {
    const user = userEvent.setup();
    const undone: UndoResponse = {
      policies: [{ policy_id: 15, policy_name: "old-sales", grants_deleted: 1, rules_deleted: 0 }],
      compile: COMPILE,
      notices: [UNDO_NOTICE],
    };
    vi.mocked(undoServicePolicyMigration).mockResolvedValue(undone);
    renderPage();
    const row = await rowOf("old-sales");
    await user.click(within(row).getByRole("button", { name: "Undo migration of old-sales" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Undo migration" }));
    expect(undoServicePolicyMigration).toHaveBeenCalledWith(7, ["old-sales"]);
    expect(await screen.findByText(UNDO_NOTICE)).toBeInTheDocument();
  });

  it("shows an undo 409 in the server's words, with the list of what changed", async () => {
    const user = userEvent.setup();
    const changes =
      'grant g-1, created by the migration of "old-sales", has been edited; the original policy "old-sales" (id 15) has been edited or switched on since it was migrated';
    vi.mocked(undoServicePolicyMigration).mockRejectedValue(
      new ApiError(
        409,
        `conflict: undo refused, nothing was changed: ${changes}. Undo deletes exactly what the migration created and restores exactly what it switched off; reconcile these by hand (or revert the edits) first.`,
      ),
    );
    renderPage();
    const row = await rowOf("old-sales");
    await user.click(within(row).getByRole("button", { name: "Undo migration of old-sales" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Undo migration" }));

    const alert = await screen.findByText(/undo refused, nothing was changed/);
    expect(alert).toHaveTextContent('grant g-1, created by the migration of "old-sales", has been edited');
    expect(alert).toHaveTextContent('the original policy "old-sales" (id 15) has been edited or switched on');
    expect(alert.textContent).not.toMatch(/^conflict: /);
  });
});

describe("MigrateTab — a policy:read account", () => {
  it("reads every verdict but is offered no selection, preview, migrate or undo", async () => {
    renderPage(READER);
    const row = await rowOf("unregistered-catalog");
    expect(within(row).getByText(CATALOG_BLOCKER)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Preview migration" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Migrate" })).toBeNull();
    expect(screen.queryByRole("button", { name: /undo/i })).toBeNull();
    expect(screen.getByText(/needs the policy:update permission/)).toBeInTheDocument();
  });
});

describe("MigrateTab — in the tab frame", () => {
  it("has no heading of its own and no back link", async () => {
    renderPage();
    await rowOf("allow-analysts");
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("after a written migration, links to Recent changes on the Overview", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));
    await waitFor(() => expect(migrateButton()).toBeEnabled());
    expect(screen.queryByRole("link", { name: "See it in Recent changes" })).toBeNull();

    vi.mocked(migrateServicePolicies).mockResolvedValueOnce(proved({ dry_run: false, committed: true }));
    await user.click(migrateButton());
    const link = await screen.findByRole("link", { name: "See it in Recent changes" });
    expect(link).toHaveAttribute("href", "/governance#recent-changes");
    await user.click(link);
    expect(screen.getByTestId("where")).toHaveTextContent("/governance#recent-changes");
  });

  it("a migration the gate did not prove at commit time offers no such link", async () => {
    const user = userEvent.setup();
    renderPage();
    await rowOf("allow-analysts");
    await user.click(screen.getByRole("checkbox", { name: "Select allow-analysts" }));
    await user.click(screen.getByRole("button", { name: "Preview migration" }));
    await waitFor(() => expect(migrateButton()).toBeEnabled());
    vi.mocked(migrateServicePolicies).mockResolvedValueOnce(proved({ dry_run: false, committed: false, equivalent: false }));
    await user.click(migrateButton());
    await waitFor(() => expect(migrateButton()).toBeDisabled());
    expect(screen.queryByRole("link", { name: "See it in Recent changes" })).toBeNull();
  });
});

describe("MigrateTab — on the engine page", () => {
  it("the Migrate tab classifies the engine's profile by its id", async () => {
    serve({
      [`/perm/engines/${E_TR}/grants`]: ok(grantsResponse({ profile: { serviceId: 7, name: "trino-prod" } })),
      "/policies/services": ok([
        profileDto("trino-prod", {
          id: 7,
          engines: [{ id: E_TR, name: "lake-trino", status: "running", origin: "provisioned", type: "trino" }],
        }),
      ]),
      "/policies?service=trino-prod": ok([policy(11, "operator", { name: "allow-analysts", service: "trino-prod" })]),
      [`/governance/engines/${E_TR}/plugin`]: ok(pluginResponse()),
    });
    openArea(`/governance/engines/${E_TR}?tab=migrate`, ADMIN);
    expect(await screen.findByText("allow-analysts", { selector: "td span" }, { timeout: 3000 })).toBeInTheDocument();
    expect(classifyServicePolicies).toHaveBeenCalledWith(7);
    vi.unstubAllGlobals();
  });
});

