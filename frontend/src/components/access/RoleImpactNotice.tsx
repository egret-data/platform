import { type AccessRole } from "@/api/access";

/**
 * Transitive count of roles whose `parentRoleId` chain reaches `role.id`,
 * walking the flat roles list. Mirrors the walk style of
 * `RolesPage`'s `cyclicParentIds`: a depth cap (`hops < 64`) guards against
 * a cycle turning this into an infinite loop.
 */
function countDescendants(roleId: string, roles: AccessRole[]): number {
  const parentOf = new Map(roles.map((r) => [r.id, r.parentRoleId ?? null]));
  let count = 0;
  for (const r of roles) {
    if (r.id === roleId) continue;
    let cur: string | null | undefined = r.id;
    let hops = 0;
    while (cur && hops < 64) {
      cur = parentOf.get(cur) ?? null;
      if (cur === roleId) {
        count += 1;
        break;
      }
      hops += 1;
    }
  }
  return count;
}

/**
 * Blast-radius summary shown before an admin edits or deletes a role: how
 * many assignees currently hold it, and how many roles inherit from it
 * (transitively). Pure client-side — derived entirely from data already on
 * the page (`assigneeCount` + the flat roles list), no fetch.
 */
export function RoleImpactNotice({
  role,
  roles,
}: {
  role: AccessRole;
  roles: AccessRole[];
}) {
  const descendantCount = countDescendants(role.id, roles);
  const hasAssignees = role.assigneeCount > 0;
  const hasDescendants = descendantCount > 0;

  return (
    <div className="space-y-1">
      {hasAssignees && (
        <p
          className="rounded-md px-2 py-1 font-medium"
          style={{
            backgroundColor: "var(--highlight)",
            color: "var(--highlight-foreground)",
          }}
        >
          Held by {role.assigneeCount} assignee{role.assigneeCount === 1 ? "" : "s"}.
        </p>
      )}
      {hasDescendants && (
        <p>
          {descendantCount} role{descendantCount === 1 ? "" : "s"} inherit from this role and
          will lose the inherited permissions.
        </p>
      )}
      {!hasAssignees && !hasDescendants && <p>No assignees or inheriting roles.</p>}
    </div>
  );
}

export default RoleImpactNotice;
