import { Fragment } from "react";
import { Link } from "react-router-dom";
import { FileCode2 } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { TagPolicyRow } from "@/api/generated/model";
import { useControlPlaneGovernanceTagHandwritten } from "@/api/generated/control-plane-governance/control-plane-governance";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { editorUrl } from "@/routes/governance/engines/engineUrl";
import { HandWrittenItems } from "@/routes/governance/handwritten/HandWrittenItems";
import { DENY_ALL_ELSE, policyTypeWord } from "@/routes/governance/handwritten/handwrittenText";
import { UnreadablePolicies } from "@/routes/governance/handwritten/UnreadablePolicies";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";

export const TAG_POLICIES_NEED_POLICY_READ =
  "Hand-written tag policies are read with the policy:read permission, which this account does not have.";

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/**
 * Where a tag policy is edited (spec §5.5): the By-tag editor of the first
 * engine that downloads it, or, when no engine is bound, of its profile's page.
 */
function editHref(p: TagPolicyRow): string | null {
  const engine = [...p.engines].sort(byName)[0];
  const profile = [...p.profiles].sort(byName)[0];
  const subject = engine ? { engineId: engine.id } : profile ? { profile: profile.name } : null;
  return subject ? `${editorUrl(subject, p.policyType, p.id)}?scope=tag` : null;
}

/**
 * Hand-written (spec §5.5, Q4), from route D: every hand-written policy of a
 * usable tag service whose tag resource matches this classification's name
 * through the tag service-def's matcher. A row is a match on the tag name, not
 * a decision: disabled policies, deny-all-else and constructs egret does not
 * evaluate are stated (CL9), and group and role items reach nobody (Q1).
 * Read with `policy:read`; without it the tab says so and asks nothing.
 */
export function HandWrittenTagTab({ tagId }: { tagId: string }) {
  const canRead = usePolicyPermissions()("policy:read");
  if (!canRead) return <p className="text-sm text-muted-foreground">{TAG_POLICIES_NEED_POLICY_READ}</p>;
  return <Body tagId={tagId} />;
}

function Body({ tagId }: { tagId: string }) {
  const q = useControlPlaneGovernanceTagHandwritten(tagId, { query: { retry: false } });
  if (q.isError) {
    const message = (q.error as unknown as ApiError | null)?.message;
    return <TabError what="hand-written tag policies" onRetry={() => void q.refetch()} detail={message} />;
  }
  if (q.data?.status !== 200) return <ListSkeleton rows={3} cols={4} />;
  const d = q.data.data;
  if (d.state === "no_tag_service") {
    return (
      <p className="text-sm text-muted-foreground">
        No engine profile has a tag service, so no engine downloads tag policies.
      </p>
    );
  }
  const policies = [...d.policies].sort((a, b) => a.service.localeCompare(b.service) || byName(a, b));

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Hand-written tag policies whose tag matches this classification&apos;s name. A listed policy names the tag; it
        may still be disabled or outranked on the engine.
      </p>
      {d.sameName.map((s) => (
        <p key={s.id} className="text-sm">
          {"Tag "}
          <Link to={tagUrl(s.id)} className="font-mono text-primary hover:underline">
            {s.key}
          </Link>
          {" has this name too; engine tag policies on this name apply to both."}
        </p>
      ))}
      {policies.length === 0 ? (
        // With unreadable policies a clean "none" would be an overclaim: they may name this tag.
        <EmptyState
          icon={FileCode2}
          title={
            d.unreadable.length > 0
              ? "No readable hand-written tag policy names this classification."
              : "No hand-written tag policy names this classification."
          }
        />
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table aria-label="Hand-written tag policies" className={STACKED_TABLE}>
            <TableHeader>
              <TableRow>
                <TableHead>Policy</TableHead>
                <TableHead>Engines</TableHead>
                <TableHead>Items</TableHead>
                <TableHead>State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {policies.map((p) => {
                const href = editHref(p);
                const engines = [...p.engines].sort(byName);
                const profiles = [...p.profiles].sort(byName);
                return (
                  <TableRow key={`${p.service}/${p.id}`}>
                    <TableCell className="whitespace-normal align-top">
                      <span className="block">
                        {href ? (
                          <Link to={href} className="break-all font-medium text-primary hover:underline">
                            {p.name}
                          </Link>
                        ) : (
                          <span className="break-all font-medium">{p.name}</span>
                        )}
                      </span>
                      <span className="block text-xs text-muted-foreground">
                        {policyTypeWord(p.policyType)} · {p.service}
                      </span>
                      <span className="block break-all font-mono text-xs text-muted-foreground">{p.resourceSummary}</span>
                      {p.throughPattern ? (
                        <span className="block text-xs text-muted-foreground">
                          Names this classification through a pattern.
                        </span>
                      ) : null}
                    </TableCell>
                    <TableCell data-label="Engines" className="whitespace-normal align-top text-sm">
                      {engines.length > 0
                        ? engines.map((e) => e.name).join(", ")
                        : profiles.map((pr, i) => (
                            <Fragment key={pr.name}>
                              {i > 0 ? " " : null}
                              {`No engine downloads profile ${pr.name}.`}
                            </Fragment>
                          ))}
                    </TableCell>
                    <TableCell data-label="Items" className="whitespace-normal align-top">
                      <HandWrittenItems items={p.items} />
                    </TableCell>
                    <TableCell data-label="State" className="whitespace-normal align-top text-sm">
                      <Badge variant={p.enabled ? "secondary" : "outline"}>{p.enabled ? "Enabled" : "Disabled"}</Badge>
                      {p.denyAllElse ? <span className="mt-1 block text-xs">{DENY_ALL_ELSE}</span> : null}
                      {p.unmodelled.map((u) => (
                        <span key={u} className="mt-1 block text-xs">
                          {u}
                        </span>
                      ))}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </div>
      )}
      <UnreadablePolicies policies={d.unreadable} what="tag policy" />
    </div>
  );
}
