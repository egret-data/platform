import { Link } from "react-router-dom";
import { ShieldCheck } from "lucide-react";
import type { ClearanceFact, EgretClearance, EngineClearance, MembershipScope, Via } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import type { Principal } from "@/routes/governance/people/principalData";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import { TERM } from "@/routes/governance/glossary";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";
import { cn } from "@/lib/utils";

/** The verbatim sentence for a gate that names only groups (spec §2): the compiler emits nothing for it. */
export const GROUPS_ONLY_GATE = "Engines do not enforce this gate (it names only groups)";

/**
 * The engines' answer, in words, as the server gives it. The compiler enforces
 * only a gate's roles, as a deny excepting the roles' direct holders with a
 * username (`clearance.rs:33-58`); a gate naming only groups compiles to
 * nothing (`clearance.rs:61-70`).
 */
export function enginesText(e: EngineClearance): string {
  switch (e.kind) {
    case "cleared":
      return `Cleared through role ${e.role}`;
    case "not_enforced_groups_only":
      return GROUPS_ONLY_GATE;
    case "not_cleared":
      switch (e.because) {
        case "held_through_group":
          return "Not cleared: the required role is held through a group, and engines count only roles held directly";
        case "expired":
          return "Not cleared: the required role has expired. Engines may apply it until the next recompile.";
        case "no_username":
          return "Not cleared: no username yet";
        case "groups_not_enforced":
          return "Not cleared: engines enforce only the gate's roles, not its groups";
        default:
          return "Not cleared";
      }
  }
}

function viaText(via: Via, kind: Principal["kind"]): string {
  switch (via.kind) {
    case "itself":
      return `Cleared: the gate names this ${kind}`;
    case "role":
      return `Cleared through role ${via.name}`;
    case "group":
      return `Cleared through group ${via.name}`;
    case "group_role":
      return `Cleared through role ${via.role}, held by group ${via.group}`;
    // The server clears through a role or a group only; any other path reads as what it names.
    case "workspace":
    case "group_workspace":
      return `Cleared through workspace ${via.name}`;
    case "public":
      return "Cleared through public";
  }
}

/**
 * How far a clearing role reaches in egret: a role held for a workspace counts
 * only for that workspace's objects, and one held on an object only for it
 * (and, for a folder, what is inside it) (`crates/egret-auth/src/grants.rs`,
 * `grants_for`). A platform role, or no scope, reaches every object.
 */
function scopeNote(scope: MembershipScope | null | undefined): string {
  if (!scope || scope.type === "platform") return "";
  if (scope.type === "workspace") {
    return `, for workspace ${scope.name}'s objects only: a workspace role clears only that workspace's objects`;
  }
  return `, for ${scope.objectType ?? "object"} ${scope.id} only: an object role clears only that object`;
}

/**
 * egret's own answer (its object-access check honours both halves of the gate
 * and exempts platform admins, `crates/egret-auth/src/pdp/templates.rs`), in
 * words, as the server gives it.
 */
export function egretText(e: EgretClearance, fact: ClearanceFact, kind: Principal["kind"]): string {
  if (e.kind === "not_cleared") return "Not cleared";
  const exempt =
    e.via.kind === "role" && e.via.name === "platform_admin" && !fact.requiredRoles.includes("platform_admin")
      ? " (platform admins are exempt from every gate)"
      : "";
  return `${viaText(e.via, kind)}${exempt}${scopeNote(e.scope)}`;
}

/**
 * A principal's Clearances tab (spec §2): every gated classification (tag)
 * with two answers kept apart, as the server computes them on the principal's
 * read: **Engines**, and **egret objects**. A tag links to its classification
 * page only for a viewer with `tag:read`, which that page's route requires.
 */
export function ClearancesTab({ principal }: { principal: Principal }) {
  const can = usePolicyPermissions();
  const canReadTags = can("tag:read");
  const facts = principal.data.clearances;

  if (facts.length === 0) return <EmptyState icon={ShieldCheck} title="No classification is gated" />;

  const sorted = [...facts].sort((a, b) => a.tagName.localeCompare(b.tagName));
  return (
    <Table aria-label="Clearances">
      <TableHeader>
        <TableRow>
          <TableHead>{TERM.classification}</TableHead>
          <TableHead>Gate requires</TableHead>
          <TableHead>Engines</TableHead>
          <TableHead>egret objects</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sorted.map((f) => (
          <TableRow key={f.tagId}>
            <TableCell className="whitespace-normal">
              {canReadTags ? (
                <Link
                  to={tagUrl(f.tagId, "clearance")}
                  className={cn(ENTITY_LINK_CLASS, "text-sm")}
                >
                  {f.tagName}
                </Link>
              ) : (
                <span className="text-sm font-medium">{f.tagName}</span>
              )}
            </TableCell>
            <TableCell className="whitespace-normal">
              <div className="space-y-0.5 text-sm">
                {f.requiredRoles.length > 0 ? <p>{`Roles: ${f.requiredRoles.join(", ")}`}</p> : null}
                {f.requiredGroups.length > 0 ? <p>{`Groups: ${f.requiredGroups.join(", ")}`}</p> : null}
              </div>
            </TableCell>
            <TableCell className="whitespace-normal text-sm">{enginesText(f.engines)}</TableCell>
            <TableCell className="whitespace-normal text-sm">{egretText(f.egret, f, principal.kind)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
