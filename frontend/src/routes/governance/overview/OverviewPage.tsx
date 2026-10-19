import type { ReactNode } from "react";
import { useControlPlaneGovernanceAttention } from "@/api/generated/control-plane-governance/control-plane-governance";
import { useAuth } from "@/auth/useAuth";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { AttentionSection } from "@/routes/governance/overview/AttentionSection";
import { CountsStrip } from "@/routes/governance/overview/CountsStrip";
import { RecentChanges } from "@/routes/governance/overview/RecentChanges";

function Section({
  id,
  anchor,
  title,
  children,
}: {
  id: string;
  /** The section's own id, for a link from elsewhere (`/governance#recent-changes`). */
  anchor?: string;
  title: string;
  children: ReactNode;
}) {
  return (
    <section id={anchor} aria-labelledby={id} className="mt-6 flex flex-col gap-3">
      <h2 id={id} className="text-base font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}

/**
 * The Governance Overview (DG redesign #3, spec
 * `docs/en/specs/2026-10-18-dg-overview-design.md` §2): the Governance index.
 * Each section has its own loading, error and empty state, so one failing
 * request never blanks the page (FE-STATE-1/2/3).
 *
 * Lazy (FE-ROUTE-2): `App.tsx` reaches it only through `lazy()`, and
 * `npm run test:bundle:governance-overview` checks it stays out of the entry
 * chunk.
 */
export default function OverviewPage() {
  const { me } = useAuth();
  const canReadTags =
    !!me && (me.platform_roles.includes("platform_admin") || me.permissions.includes("tag:read"));
  const attention = useControlPlaneGovernanceAttention({ query: { retry: false } });
  const data = attention.data?.status === 200 ? attention.data.data : undefined;
  return (
    <ListPageShell
      title="Overview"
      description="What needs attention in governance, and what changed recently."
    >
      {data ? <CountsStrip counts={data.counts} /> : null}
      <Section id="overview-attention" title="Needs attention">
        <AttentionSection
          isLoading={attention.isLoading}
          isError={attention.isError}
          error={attention.error}
          data={data}
          onRetry={() => void attention.refetch()}
          canReadTags={canReadTags}
        />
      </Section>
      <Section id="overview-changes" anchor="recent-changes" title="Recent changes">
        <RecentChanges />
      </Section>
    </ListPageShell>
  );
}
