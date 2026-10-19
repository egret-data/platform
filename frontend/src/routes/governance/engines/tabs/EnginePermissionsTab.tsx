import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { KeyRound, Pencil, Trash2 } from "lucide-react";
import type { CapabilityView, EngineGrantView, ManagedEntry, ManagedKind, PermPrincipal } from "@/api/generated/model";
import {
  getPermListAllEngineGrantsQueryKey,
  getPermListEngineGrantsQueryKey,
  usePermDeleteEngineGrant,
  usePermListEngineCapabilities,
} from "@/api/generated/perm/perm";
import { getPolicyAdminListPoliciesQueryKey } from "@/api/generated/policy-admin/policy-admin";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { IconButton } from "@/components/ui/icon-button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { ACTIONS_CELL, Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { principalLinkFor, principalUrl } from "@/routes/governance/people/peopleUrl";
import { engineUrl } from "@/routes/governance/engines/engineUrl";
import type { TabContext } from "@/routes/governance/engines/PageFrame";

export const NO_PROFILE_SENTENCE =
  "This engine downloads no policy profile, so nothing can be granted on it. Bind a profile in Control plane.";
export const NOTHING_GRANTED = "No engine permission has been granted on this engine.";

/** The kinds of egret-seeded engine policy (spec §6.1). */
export const MANAGED_KIND_LABEL: Record<ManagedKind, string> = {
  user_baseline: "Every user's baseline",
  delegator: "egret's delegator",
  admin_all_access: "Platform admins: all access",
  default_all_access: "Group admin: all access (seeded defaults)",
};

/** A grant principal as text: `user ua`, `group analysts`, `role etl`, `workspace w1`. */
export function principalText(p: PermPrincipal): string {
  return p.kind === "workspace_members" ? `workspace ${p.ref}` : `${p.kind} ${p.ref}`;
}

export function PrincipalLinks({ principals }: { principals: PermPrincipal[] }) {
  return (
    <ul role="list" className="flex flex-wrap gap-x-2 gap-y-0.5">
      {principals.map((p) => {
        const link = principalLinkFor(p);
        const text = principalText(p);
        return (
          <li key={`${p.kind}:${p.ref}`}>
            {link ? (
              <Link to={principalUrl(link)} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
                {text}
              </Link>
            ) : (
              text
            )}
          </li>
        );
      })}
    </ul>
  );
}

/**
 * A grant's values, one per resource of its capability in the catalogue's
 * order. An impersonation target (`username`) is stored by id: the server
 * sends its current username in `values` and its id in `targetUserId`; a
 * target with no username is missing from `values` (spec premise 2).
 */
function GrantValues({ grant, capability }: { grant: EngineGrantView; capability: CapabilityView | undefined }) {
  const resources = capability?.resources ?? grant.values.map((_, i) => ({ key: String(i), valueKind: "identifier" }));
  // A target with no username is the one value `values` lacks.
  const missingUsername = grant.values.length < resources.filter((r) => r.valueKind !== "fixed").length;
  const parts: ReactNode[] = [];
  let next = 0;
  for (const r of resources) {
    if (r.valueKind === "fixed") {
      parts.push(<span key={r.key}>All (*)</span>);
    } else if (r.valueKind === "username" && missingUsername) {
      parts.push(<span key={r.key}>a user with no username</span>);
    } else if (r.valueKind === "username") {
      const username = grant.values[next++];
      const to = grant.targetUserId
        ? principalUrl({ kind: "user", userId: grant.targetUserId })
        : principalUrl({ kind: "user", username });
      parts.push(
        <Link key={r.key} to={to} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
          {username}
        </Link>,
      );
    } else {
      parts.push(
        <span key={r.key} className="font-mono">
          {grant.values[next++] ?? ""}
        </span>,
      );
    }
  }
  return <span className="flex flex-wrap items-center gap-x-2">{parts}</span>;
}

function GroupSection({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="space-y-1">
      <h3 id={headingId} className="text-sm font-semibold">
        {title}
      </h3>
      {description ? <p className="text-xs text-muted-foreground">{description}</p> : null}
      {children}
    </section>
  );
}

function resourcesText(resources: ManagedEntry["resources"]): string[] {
  return Object.entries(resources).map(([k, v]) => `${k}=${v.join(",")}`);
}

/**
 * The Engine permissions tab (spec §6.1, EP4): what egret grants on this
 * engine through F11's routes, grouped by capability; the engine policies egret
 * seeds itself (read-only); and what this engine type cannot grant. Writes
 * need `policy:update`. `onGrant`/`onEdit` open the grant dialog.
 */
export function EnginePermissionsTab({
  ctx,
  onEdit,
  toolbar,
}: {
  ctx: TabContext;
  onEdit?: (grant: EngineGrantView) => void;
  /** Rendered above the sections when the engine has a profile (Grant, Check). */
  toolbar?: ReactNode;
}) {
  const can = usePolicyPermissions();
  const canWrite = can("policy:update");
  const capsQ = usePermListEngineCapabilities();
  const [search] = useSearchParams();
  const highlighted = search.get("grant");
  const queryClient = useQueryClient();
  const [deleting, setDeleting] = useState<EngineGrantView | null>(null);
  const engineId = ctx.engine?.id ?? "";
  const engineType = ctx.engine?.type ?? "";

  const deleteGrant = usePermDeleteEngineGrant({
    mutation: {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: getPermListEngineGrantsQueryKey(engineId) });
        void queryClient.invalidateQueries({ queryKey: getPermListAllEngineGrantsQueryKey() });
        if (ctx.profile) {
          void queryClient.invalidateQueries({
            queryKey: getPolicyAdminListPoliciesQueryKey({ service: ctx.profile.name }),
          });
        }
        toast.success("Engine permission deleted.");
      },
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The engine permission could not be deleted.")),
    },
  });

  const grants = ctx.grants;
  if (!grants) return null;
  if (!grants.profile) {
    return (
      <Alert>
        <AlertDescription>{NO_PROFILE_SENTENCE}</AlertDescription>
      </Alert>
    );
  }

  const catalogue = capsQ.data?.status === 200 ? capsQ.data.data.engines.find((e) => e.engineType === engineType) : undefined;
  const capabilities = catalogue?.capabilities ?? [];
  const capOf = (id: string) => capabilities.find((c) => c.id === id);

  // Grouped in the catalogue's order; a capability the catalogue does not
  // name (it failed to load, or a newer server) follows, by id.
  const order = new Map(capabilities.map((c, i) => [c.id, i]));
  const groups = new Map<string, EngineGrantView[]>();
  for (const g of grants.grants) groups.set(g.capability, [...(groups.get(g.capability) ?? []), g]);
  const groupIds = [...groups.keys()].sort((a, b) => {
    const ia = order.get(a) ?? Number.MAX_SAFE_INTEGER;
    const ib = order.get(b) ?? Number.MAX_SAFE_INTEGER;
    return ia !== ib ? ia - ib : a.localeCompare(b);
  });
  for (const rows of groups.values()) rows.sort((a, b) => a.policyName.localeCompare(b.policyName));

  return (
    <div className="space-y-6">
      {toolbar}
      {capsQ.isError ? <TabError what="capability catalogue" onRetry={() => void capsQ.refetch()} /> : null}
      <Section title="Granted">
        {grants.grants.length === 0 ? (
          <EmptyState icon={KeyRound} title={NOTHING_GRANTED} />
        ) : (
          <div className="space-y-4">
            {groupIds.map((capId) => {
              const cap = capOf(capId);
              return (
                <GroupSection key={capId} title={cap?.label ?? capId} description={cap?.description}>
                  <Table className={STACKED_TABLE}>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Values</TableHead>
                        <TableHead>Granted to</TableHead>
                        <TableHead>Policy</TableHead>
                        {canWrite ? <TableHead className={ACTIONS_CELL}>Actions</TableHead> : null}
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {groups.get(capId)!.map((g) => (
                        <GrantRow
                          key={g.id}
                          grant={g}
                          capability={cap}
                          engineId={engineId}
                          highlighted={g.id === highlighted}
                          canWrite={canWrite}
                          onEdit={onEdit}
                          onDelete={() => setDeleting(g)}
                        />
                      ))}
                    </TableBody>
                  </Table>
                </GroupSection>
              );
            })}
          </div>
        )}
      </Section>

      <Section title="Managed by egret">
        {grants.managed.length === 0 ? (
          <p className="text-sm text-muted-foreground">egret seeds no engine policy on this profile.</p>
        ) : (
          <Table className={STACKED_TABLE}>
            <TableHeader>
              <TableRow>
                <TableHead>Kind</TableHead>
                <TableHead>Capability</TableHead>
                <TableHead>Resources</TableHead>
                <TableHead>Access</TableHead>
                <TableHead>Users and groups</TableHead>
                <TableHead>State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {grants.managed.map((m) => (
                <TableRow key={m.policyId}>
                  <TableCell className="font-medium">{MANAGED_KIND_LABEL[m.kind] ?? m.kind}</TableCell>
                  <TableCell data-label="Capability">{m.capability ? (capOf(m.capability)?.label ?? m.capability) : ""}</TableCell>
                  <TableCell data-label="Resources">
                    <ul role="list">
                      {resourcesText(m.resources).map((r) => (
                        <li key={r} className="font-mono text-xs">
                          {r}
                        </li>
                      ))}
                    </ul>
                  </TableCell>
                  <TableCell data-label="Access">
                    <ul role="list">
                      {m.accesses.map((a) => (
                        <li key={a}>{a}</li>
                      ))}
                    </ul>
                  </TableCell>
                  <TableCell data-label="Users and groups">
                    <ul role="list">
                      {m.users.map((u) => (
                        <li key={`u:${u}`}>{`user ${u}`}</li>
                      ))}
                      {m.groups.map((g) => (
                        <li key={`g:${g}`}>{`group ${g}`}</li>
                      ))}
                    </ul>
                  </TableCell>
                  <TableCell data-label="State">{m.enabled ? "Enabled" : "Disabled"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>

      {catalogue && catalogue.notGrantable.length > 0 ? (
        <Section title="Not grantable here">
          <ul role="list" className="space-y-1 text-sm">
            {catalogue.notGrantable.map((n) => (
              <li key={n.what}>
                <span className="font-medium">{n.what}</span>
                <span className="text-muted-foreground">{" — "}</span>
                <span>{n.why}</span>
              </li>
            ))}
          </ul>
          <p className="text-sm">
            {"A hand-written policy in "}
            <Link
              to={engineUrl(engineId, "advanced")}
              className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring"
            >
              Advanced
            </Link>
            {" can grant it."}
          </p>
        </Section>
      ) : capsQ.isLoading ? (
        <ListSkeleton rows={2} cols={2} />
      ) : null}

      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title="Delete engine permission"
        description={
          deleting
            ? `Delete ${deleting.policyName}? egret recompiles the profile; the engine drops it at its next plugin download.`
            : ""
        }
        onConfirm={() => {
          if (deleting) deleteGrant.mutate({ id: deleting.id });
        }}
      />
    </div>
  );
}

function GrantRow({
  grant,
  capability,
  engineId,
  highlighted,
  canWrite,
  onEdit,
  onDelete,
}: {
  grant: EngineGrantView;
  capability: CapabilityView | undefined;
  engineId: string;
  highlighted: boolean;
  canWrite: boolean;
  onEdit?: (grant: EngineGrantView) => void;
  onDelete: () => void;
}) {
  const ref = useRef<HTMLTableRowElement>(null);
  useEffect(() => {
    if (highlighted) ref.current?.scrollIntoView({ block: "center" });
  }, [highlighted]);
  return (
    <TableRow ref={ref} aria-current={highlighted ? "true" : undefined} className={highlighted ? "bg-highlight" : undefined}>
      <TableCell>
        <GrantValues grant={grant} capability={capability} />
      </TableCell>
      <TableCell data-label="Granted to">
        <PrincipalLinks principals={grant.principals} />
      </TableCell>
      <TableCell data-label="Policy">
        <span className="font-mono text-xs">{grant.policyName}</span>
        {grant.engineId !== engineId ? (
          <span className="block text-xs text-muted-foreground">{`Written on engine ${grant.engineName}`}</span>
        ) : null}
      </TableCell>
      {canWrite ? (
        <TableCell className={ACTIONS_CELL}>
          <div className="flex gap-1">
            {onEdit ? (
              <IconButton aria-label={`Edit engine permission ${grant.policyName}`} onClick={() => onEdit(grant)}>
                <Pencil aria-hidden />
              </IconButton>
            ) : null}
            <IconButton aria-label={`Delete engine permission ${grant.policyName}`} onClick={onDelete}>
              <Trash2 aria-hidden />
            </IconButton>
          </div>
        </TableCell>
      ) : null}
    </TableRow>
  );
}
