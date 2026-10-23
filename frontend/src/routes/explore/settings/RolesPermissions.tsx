import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { Search } from "lucide-react";

import { useWorkspaceRoleCatalog, type WsPerm, type WsRole } from "@/api/workspaceAccess";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { errorStatus } from "@/lib/httpStatus";
import { Forbidden } from "@/routes/Forbidden";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";

/** Built-in role names, seeded on install — mirrors RolesPage's own copy. */
const SEEDED_ROLE_NAMES = new Set(["ws_admin", "ws_editor", "ws_viewer"]);
/** Display order for the seeded roles (most-privileged first). */
const SEED_ORDER = ["ws_admin", "ws_editor", "ws_viewer"];

function isSeeded(role: WsRole): boolean {
  return SEEDED_ROLE_NAMES.has(role.name);
}

/** Seeded roles first (in privilege order), then everything else alphabetically. */
function compareRoles(a: WsRole, b: WsRole): number {
  const ia = SEED_ORDER.indexOf(a.name);
  const ib = SEED_ORDER.indexOf(b.name);
  if (ia !== -1 || ib !== -1) {
    if (ia === -1) return 1;
    if (ib === -1) return -1;
    return ia - ib;
  }
  return a.name.localeCompare(b.name);
}

function permMatches(p: WsPerm, q: string): boolean {
  return (
    p.key.toLowerCase().includes(q) ||
    (p.description ?? "").toLowerCase().includes(q) ||
    (p.group ?? "").toLowerCase().includes(q)
  );
}

/** A role plus the permissions to show for it under the current search. */
type FilteredRole = { role: WsRole; perms: WsPerm[]; inherited: WsPerm[] };

/** What a holder of the role gets: its own permissions plus the inherited ones. */
function totalCount(role: WsRole): number {
  return role.permissions.length + role.inherited_permissions.length;
}

/** Permissions inside an expanded role, clustered by their group so large
 *  roles (ws_admin / ws_editor) stay scannable instead of being one long list. */
function PermList({ perms }: { perms: WsPerm[] }) {
  const byGroup = useMemo(() => {
    const map = new Map<string, WsPerm[]>();
    for (const p of perms) {
      const g = p.group || "Other";
      const bucket = map.get(g);
      if (bucket) bucket.push(p);
      else map.set(g, [p]);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [perms]);

  if (perms.length === 0) {
    return <p className="text-sm text-muted-foreground">No permissions.</p>;
  }

  return (
    <div className="space-y-4">
      {byGroup.map(([group, ps]) => (
        <div key={group}>
          <p className="mb-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground">
            {group}
          </p>
          <ul className="space-y-1">
            {ps.map((p) => (
              <li key={p.key} className="flex flex-wrap items-baseline gap-x-2 text-sm">
                <span className="font-mono text-xs text-foreground">{p.key}</span>
                {p.description && (
                  <span className="text-muted-foreground">— {p.description}</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

/**
 * Read-only view of workspace roles and the permissions each one grants.
 *
 * Roles themselves are defined platform-wide (Control Plane → Access control)
 * and merely apply per-workspace, so there's nothing to edit here — the page
 * answers "what can each role do?". Roles are shown as an accordion (one row
 * each, expandable to its permission list) with a single search across every
 * role and permission, rather than a wall of one-table-per-role cards.
 */
export default function RolesPermissions() {
  const ws = useParams().workspace!;
  const q = useWorkspaceRoleCatalog(ws);

  const roles = q.data ?? [];
  const forbidden = errorStatus(q.error) === 403;
  const nameById = useMemo(() => new Map(roles.map((r) => [r.id, r.name])), [roles]);

  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string[]>([]);
  const [hideEmpty, setHideEmpty] = useState(false);

  const sorted = useMemo(() => [...roles].sort(compareRoles), [roles]);

  const seededCount = useMemo(() => roles.filter(isSeeded).length, [roles]);
  const summaryItems = [
    { label: "roles", value: roles.length },
    { label: "built-in", value: seededCount },
    { label: "custom", value: roles.length - seededCount },
  ];

  const filtered: FilteredRole[] = useMemo(() => {
    const needle = query.trim().toLowerCase();
    let out: FilteredRole[];
    if (!needle) {
      out = sorted.map((role) => ({
        role,
        perms: role.permissions,
        inherited: role.inherited_permissions,
      }));
    } else {
      out = [];
      for (const role of sorted) {
        const roleHit =
          role.name.toLowerCase().includes(needle) ||
          (role.description ?? "").toLowerCase().includes(needle);
        const perms = roleHit
          ? role.permissions
          : role.permissions.filter((p) => permMatches(p, needle));
        const inherited = roleHit
          ? role.inherited_permissions
          : role.inherited_permissions.filter((p) => permMatches(p, needle));
        if (roleHit || perms.length > 0 || inherited.length > 0) {
          out.push({ role, perms, inherited });
        }
      }
    }
    // "Hide empty roles" filters on the role's real permission count, not the
    // search-narrowed list, so the toggle means the same thing while searching.
    if (hideEmpty) out = out.filter((f) => totalCount(f.role) > 0);
    return out;
  }, [sorted, query, hideEmpty]);

  const searching = query.trim().length > 0;
  const filtering = searching || hideEmpty;
  // While searching, every matching role is forced open so hits are visible;
  // otherwise the user controls which rows are expanded.
  const openValue = searching ? filtered.map((f) => f.role.id) : open;

  return (
    <div className="space-y-5">
      <header className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">Roles &amp; permissions</h2>
          <p className="text-sm text-muted-foreground">
            What each workspace role can do in <span className="font-medium">{ws}</span>.
          </p>
        </div>
        <SettingsNote>
          Roles are defined platform-wide in{" "}
          <span className="font-medium">Control Plane → Access control</span> and apply to every
          workspace. This is a read-only view. The Control Plane also lists{" "}
          <span className="font-mono text-xs">data_api_invoker</span>: it is granted on one Data
          API endpoint, never as a workspace role, so it is not shown here.
        </SettingsNote>
      </header>

      {q.isLoading ? (
        <ListSkeleton rows={4} cols={3} />
      ) : forbidden ? (
        <Forbidden level="section" reason="Only a workspace admin can view the workspace roles." />
      ) : q.isError ? (
        <p className="py-6 text-sm text-destructive" role="alert">
          Couldn&apos;t load roles. {String(q.error)}
        </p>
      ) : roles.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No workspace roles defined.
        </p>
      ) : (
        <>
          <SummaryStrip items={summaryItems} />

          <div className="flex flex-wrap items-center gap-3">
            <div className="relative w-full max-w-sm">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="h-9 pl-8"
                placeholder="Search roles or permissions…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                aria-label="Search roles or permissions"
              />
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-sm text-muted-foreground">
              <Checkbox
                checked={hideEmpty}
                onCheckedChange={(v) => setHideEmpty(v === true)}
                aria-label="Hide empty roles"
              />
              Hide empty roles
            </label>
            {filtering && (
              <span className="ml-auto shrink-0 text-sm text-muted-foreground tabular-nums">
                {filtered.length} of {roles.length} role{roles.length === 1 ? "" : "s"}
              </span>
            )}
          </div>

          {filtered.length === 0 ? (
            <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
              {searching ? `No roles or permissions match “${query}”.` : "No roles to show."}
            </p>
          ) : (
            <Accordion
              type="multiple"
              value={openValue}
              onValueChange={searching ? undefined : setOpen}
              className="rounded-md border"
            >
              {filtered.map(({ role, perms, inherited }, i) => {
                const parentName = role.parent_role_id
                  ? nameById.get(role.parent_role_id)
                  : undefined;
                const count = totalCount(role);
                const inheritedCount = role.inherited_permissions.length;
                return (
                  <AccordionItem
                    key={role.id}
                    value={role.id}
                    className={i === filtered.length - 1 ? "border-0" : ""}
                  >
                    <AccordionTrigger className="px-3 hover:no-underline">
                      <div className="flex min-w-0 flex-1 items-center gap-2 pr-2">
                        <span className="min-w-0 truncate font-medium" title={role.name}>
                          {role.name}
                        </span>
                        {isSeeded(role) && (
                          <Badge variant="secondary" className="shrink-0 font-normal">
                            built-in
                          </Badge>
                        )}
                        {parentName && (
                          <Badge variant="outline" className="shrink-0 font-normal">
                            Inherits from {parentName}
                          </Badge>
                        )}
                        <span className="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">
                          {count} permission{count === 1 ? "" : "s"}
                          {inheritedCount > 0 && ` (${inheritedCount} inherited)`}
                        </span>
                      </div>
                    </AccordionTrigger>
                    <AccordionContent className="px-3">
                      {role.description && (
                        <p className="mb-3 text-sm text-muted-foreground">{role.description}</p>
                      )}
                      {(perms.length > 0 || inherited.length === 0) && <PermList perms={perms} />}
                      {inherited.length > 0 && (
                        <div className={perms.length > 0 ? "mt-4 border-t pt-3" : ""}>
                          <p className="mb-2 text-sm font-medium">
                            Inherited from {parentName ?? "its parent role"}
                          </p>
                          <PermList perms={inherited} />
                        </div>
                      )}
                    </AccordionContent>
                  </AccordionItem>
                );
              })}
            </Accordion>
          )}
        </>
      )}
    </div>
  );
}
