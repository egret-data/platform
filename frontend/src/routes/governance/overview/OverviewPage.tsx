import { ListPageShell } from "@/components/explore/ListPageShell";

/**
 * The Governance Overview (DG redesign #3, spec
 * `docs/en/specs/2026-10-18-dg-overview-design.md` §2): the Governance index.
 * Lazy (FE-ROUTE-2): `App.tsx` reaches it only through `lazy()`, and
 * `npm run test:bundle:governance-overview` checks it stays out of the entry
 * chunk.
 */
export default function OverviewPage() {
  return (
    <ListPageShell
      title="Overview"
      description="What needs attention in governance, and what changed recently."
    >
      <div data-testid="governance-overview" />
    </ListPageShell>
  );
}
