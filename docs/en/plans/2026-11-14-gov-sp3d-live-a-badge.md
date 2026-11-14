# SP3d live — Package A: migration-gate badge — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Show the full per-EP migration-gate verdict (`GET …/migration-gate`, shipped #862) as a badge on the Migrate tab, beside the existing operator-only badge.

**Architecture:** A thin wrapper in `frontend/src/api/policies.ts` over the generated `permServiceMigrationGate` fetcher + its query-key factory, mirroring the existing `serviceMigrationStatus`/`migrationStatusQueryKey` pair; a `useQuery` in `MigrateTab.tsx` invalidated on migrate/undo like the status query; and a `MigrationGateBadge` that renders `complete` or the refusal reasons (FE-side copy per `kind` — the server's `.message()` is NOT in the JSON).

**Tech Stack:** React, TanStack Query, generated orval client, vitest + Testing Library.

## Global Constraints
- Branch `feat/gov-sp3d-live` (already created off main `3013f374e`). `CARGO_TARGET_DIR` n/a (FE only). Never `git add -A` — stage by explicit path.
- Commits end `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Conventional Commits. Plain `git commit`. No secrets.
- Before changing anything under `frontend/`, read `docs/en/frontend-rules.md` (its MUST rules bind). No prettier in this tree. Regenerate the client only with `npm run gen:api`, never bare orval (Package A adds NO endpoint, so no regen is needed).
- FE gate (FE-TEST-4), run all from `frontend/`: `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, and `npm run test:bundle:governance-engines` (MigrateTab lives under governance/engines).
- No backend change: the endpoint + generated client (`permServiceMigrationGate`, `getPermServiceMigrationGateQueryKey`, `GateVerdict`, `GateRefusal`) already exist from #862.
- The existing operator-only `MigrationStatusBadge` STAYS; the new badge is the full four-condition verdict.

---

### Task 1: migration-gate badge on the Migrate tab

**Files:**
- Modify: `frontend/src/api/policies.ts` (add the wrapper + key factory + type re-exports, next to `serviceMigrationStatus` ~:298-305)
- Modify: `frontend/src/routes/governance/engines/tabs/MigrateTab.tsx` (query ~:101, invalidate ~:130, render ~:301, new component near `MigrationStatusBadge` ~:888)
- Test: `frontend/src/routes/governance/engines/__tests__/MigrateTab.test.tsx` (extend the `@/api/policies` mock + add badge tests)

**Interfaces:**
- Consumes (already generated, `@/api/generated/perm/perm`): `permServiceMigrationGate(serviceId: number) => Promise<{ data: GateVerdict }>`, `getPermServiceMigrationGateQueryKey(serviceId: number)`; models `GateVerdict { complete: boolean; refusals: GateRefusal[] }`, `GateRefusal` is a union by `kind`: `{kind:'operator_policies',pending:number,blocked:number}` | `{kind:'role_principals',count:number}` | `{kind:'workspace_grants',count:number}` | `{kind:'clearance_tags',tags:string[]}`.
- Produces (from `@/api/policies`): `serviceMigrationGate(serviceId: number): Promise<GateVerdict>`, `migrationGateQueryKey(serviceId: number)`, re-exported `type GateVerdict`, `type GateRefusal`.

- [ ] **Step 1: Add the API wrapper.** In `frontend/src/api/policies.ts`, import `permServiceMigrationGate` and `getPermServiceMigrationGateQueryKey` from `@/api/generated/perm/perm` (alongside the existing `permServiceMigrationStatus` import) and `type { GateVerdict, GateRefusal }` from `@/api/generated/model` (verify the exact model import path matches how `EpMigrationStatus` is imported in this file). Add, next to `serviceMigrationStatus`:

```ts
/** One enforcement point's full migration-gate verdict (SP3d): operator
 *  policies + role/workspace/clearance input all migrated. */
export async function serviceMigrationGate(serviceId: number): Promise<GateVerdict> {
  return (await permServiceMigrationGate(serviceId)).data as GateVerdict;
}

/** The key the verdict is cached under; a migrate or undo invalidates it. */
export function migrationGateQueryKey(serviceId: number) {
  return getPermServiceMigrationGateQueryKey(serviceId);
}
```

Re-export the types so the tab imports them from `@/api/policies` (match the file's existing `export type { … }` style): `export type { GateVerdict, GateRefusal } from "@/api/generated/model";` (or add to an existing re-export block).

- [ ] **Step 2: Write the failing badge tests.** In `MigrateTab.test.tsx`:
  - In the `vi.mock("@/api/policies", async (orig) => ({ ...(await orig()), … }))` factory (~:153), add `serviceMigrationGate: vi.fn(),` (keep `migrationGateQueryKey` real via the spread so invalidation keys match).
  - In the shared `beforeEach` (~:217-222, where `serviceMigrationStatus` is defaulted), add a default so EVERY existing test still passes and no loose matcher collides: `vi.mocked(serviceMigrationGate).mockReset().mockResolvedValue({ complete: true, refusals: [] });` (complete default renders only "fully on the model", which does NOT contain "to migrate").
  - Add three tests:

```tsx
it("shows the gate badge when migration is complete", async () => {
  vi.mocked(serviceMigrationGate).mockResolvedValue({ complete: true, refusals: [] });
  renderTab();
  expect(await screen.findByText(/fully on the model/i)).toBeInTheDocument();
});

it("lists the gate's remaining steps when incomplete", async () => {
  vi.mocked(serviceMigrationGate).mockResolvedValue({
    complete: false,
    refusals: [
      { kind: "role_principals", count: 2 },
      { kind: "clearance_tags", tags: ["pii", "phi"] },
    ],
  });
  renderTab();
  expect(await screen.findByText(/2 steps remain/i)).toBeInTheDocument();
  expect(screen.getByText(/2 role principals to migrate/i)).toBeInTheDocument();
  expect(screen.getByText(/pii, phi/i)).toBeInTheDocument();
});

it("shows no gate badge when the verdict read fails", async () => {
  vi.mocked(serviceMigrationGate).mockRejectedValue(new Error("boom"));
  renderTab();
  // the tab still renders; neither gate-badge string appears
  await screen.findByText(/who owns each policy|Search policies/i);
  expect(screen.queryByText(/fully on the model/i)).not.toBeInTheDocument();
  expect(screen.queryByText(/steps remain/i)).not.toBeInTheDocument();
});
```
  (Use the file's existing render helper — match its name, e.g. `renderTab()`/`renderMigrateTab()` — and its query-client/`retry:false` setup.)

- [ ] **Step 3: Run the tests, verify they fail.**

Run (from `frontend/`): `npm run test -- MigrateTab`
Expected: FAIL — `serviceMigrationGate` not exported / `MigrationGateBadge` absent, and the "fully on the model" / "steps remain" strings not found.

- [ ] **Step 4: Add the query, invalidation, render, and component in `MigrateTab.tsx`.**
  - Import from `@/api/policies`: `serviceMigrationGate`, `migrationGateQueryKey`, and `type GateVerdict`, `type GateRefusal` (add to the existing import block).
  - After the `status` query (~:101-107) add:

```tsx
// SP3d: the full migration-gate verdict (operator + role + workspace +
// clearance). A failed read shows no badge, like the status query.
const gate = useQuery<GateVerdict, ApiError>({
  queryKey: migrationGateQueryKey(serviceId ?? 0),
  queryFn: () => serviceMigrationGate(serviceId as number),
  enabled: serviceId !== undefined,
  retry: false,
});
```
  - In `invalidate()` (~:130, next to the `migrationStatusQueryKey` invalidate) add: `void queryClient.invalidateQueries({ queryKey: migrationGateQueryKey(serviceId) });`
  - Where `MigrationStatusBadge` is rendered (~:301) add the gate badge beneath it: `{gate.data && <MigrationGateBadge verdict={gate.data} />}`
  - Near `MigrationStatusBadge` (~:888) add:

```tsx
/** FE copy for one gate refusal — the server's `.message()` is not in the JSON. */
function gateRefusalText(r: GateRefusal): string {
  switch (r.kind) {
    case "operator_policies":
      return r.blocked > 0
        ? `${r.pending} manual policies still to migrate, ${r.blocked} blocked`
        : `${r.pending} manual policies still to migrate`;
    case "role_principals":
      return `${r.count} role principals to migrate to managed groups`;
    case "workspace_grants":
      return `${r.count} workspace grants to migrate to managed groups`;
    case "clearance_tags":
      return `${r.tags.length} gated clearance tags without a withhold rule: ${r.tags.join(", ")}`;
  }
}

/** SP3d: the full per-EP migration-gate verdict badge (beside the operator-only one). */
function MigrationGateBadge({ verdict }: { verdict: GateVerdict }) {
  if (verdict.complete) {
    return (
      <StatusBadge tone="success" icon={CheckCircle2}>
        Migration complete — fully on the model
      </StatusBadge>
    );
  }
  return (
    <div className="space-y-2">
      <StatusBadge tone="highlight">{`${verdict.refusals.length} steps remain`}</StatusBadge>
      <ul className="space-y-1 text-sm text-muted-foreground">
        {verdict.refusals.map((r) => (
          <li key={r.kind}>{gateRefusalText(r)}</li>
        ))}
      </ul>
    </div>
  );
}
```

- [ ] **Step 5: Run the badge tests, verify they pass.**

Run: `npm run test -- MigrateTab`
Expected: PASS — the three new tests AND every pre-existing MigrateTab test (the complete-default mock keeps loose matchers from colliding).

- [ ] **Step 6: Run the full FE gate.**

Run (from `frontend/`): `npm run lint && npx tsc -b && npm run test && npm run build && npm run test:bundle:governance-engines`
Expected: all pass. Fix anything flagged (the rulebook's MUST rules bind).

- [ ] **Step 7: Commit.**

```bash
git add frontend/src/api/policies.ts frontend/src/routes/governance/engines/tabs/MigrateTab.tsx frontend/src/routes/governance/engines/__tests__/MigrateTab.test.tsx
git commit -m "feat(gov): show the per-EP migration-gate verdict on the Migrate tab (SP3d)"
```

---

## Self-Review
- **Spec coverage:** Package A = "render the migration-gate verdict as a badge on the Migrate tab" → Task 1 (wrapper + query + component + tests). Screenshots are Package B; no backend change (endpoint shipped #862).
- **Sweep:** adding the `gate` query means `serviceMigrationGate` is called in every MigrateTab render, so the shared `vi.mock` + `beforeEach` MUST mock it (Step 2) or all existing tests throw; the complete-default avoids the `/to migrate/i` collision from the refusal copy. Flagged in Step 2.
- **Type consistency:** `serviceMigrationGate`/`migrationGateQueryKey`/`GateVerdict`/`GateRefusal` names match between policies.ts (Task 1 Step 1) and MigrateTab.tsx (Step 4); `GateRefusal.kind` values match the generated union.
- **No placeholders:** all code is concrete; the only "verify" notes are the exact model import path and the test render-helper name, both read from the existing file.
