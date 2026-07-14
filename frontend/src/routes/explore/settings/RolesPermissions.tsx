import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { Search } from "lucide-react";

import { useWorkspaceRoleCatalog, type WsPerm, type WsRole } from "@/api/workspaceAccess";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { errorStatus } from "@/lib/httpStatus";

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
type FilteredRole = { role: WsRole; perms: WsPerm[] };

/** The compact permission table shown inside an expanded role. */
function PermTable({ perms }: { perms: WsPerm[] }) {
  if (perms.length === 0) {
    return <p className="text-sm text-muted-foreground">No permissions.</p>;
  }
  return (
    <div className="overflow-x-auto rounded-md border">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b bg-muted/40 text-left text-xs uppercase tracking-wider text-muted-foreground">
            <th className="px-3 py-2 font-medium">Permission</th>
            <th className="px-3 py-2 font-medium">Description</th>
            <th className="px-3 py-2 font-medium">Group</th>
          </tr>
        </thead>
        <tbody>
          {perms.map((p) => (
            <tr key={p.key} className="border-b last:border-0">
              <td className="px-3 py-2 align-top">
                <span className="font-mono text-xs">{p.key}</span>
              </td>
              <td className="px-3 py-2 align-top text-muted-foreground">{p.description || "—"}</td>
              <td className="px-3 py-2 align-top">
                {p.group ? (
                  <Badge variant="outline" className="font-normal">
                    {p.group}
                  </Badge>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
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

  const sorted = useMemo(() => [...roles].sort(compareRoles), [roles]);

  const filtered: FilteredRole[] = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return sorted.map((role) => ({ role, perms: role.permissions }));
    const out: FilteredRole[] = [];
    for (const role of sorted) {
      const roleHit =
        role.name.toLowerCase().includes(needle) ||
        (role.description ?? "").toLowerCase().includes(needle);
      const perms = roleHit
        ? role.permissions
        : role.permissions.filter((p) => permMatches(p, needle));
      if (roleHit || perms.length > 0) out.push({ role, perms });
    }
    return out;
  }, [sorted, query]);

  const searching = query.trim().length > 0;
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
        <div
          className="rounded-md border border-highlight/40 bg-highlight/5 px-3 py-2 text-xs text-muted-foreground"
          role="note"
        >
          Roles are defined platform-wide in{" "}
          <span className="font-medium">Control Plane → Access control</span> and apply to every
          workspace. This is a read-only view.
        </div>
      </header>

      {q.isLoading ? (
        <ListSkeleton rows={4} cols={3} />
      ) : forbidden ? (
        <p
          className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground"
          role="alert"
        >
          Only workspace admins can view roles.
        </p>
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
          <div className="flex items-center justify-between gap-3">
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
            <span className="shrink-0 text-sm text-muted-foreground tabular-nums">
              {filtered.length} of {roles.length} role{roles.length === 1 ? "" : "s"}
            </span>
          </div>

          {filtered.length === 0 ? (
            <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
              No roles or permissions match “{query}”.
            </p>
          ) : (
            <Accordion
              type="multiple"
              value={openValue}
              onValueChange={searching ? undefined : setOpen}
              className="rounded-md border"
            >
              {filtered.map(({ role, perms }, i) => {
                const parentName = role.parent_role_id
                  ? nameById.get(role.parent_role_id)
                  : undefined;
                const count = role.permissions.length;
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
                        </span>
                      </div>
                    </AccordionTrigger>
                    <AccordionContent className="px-3">
                      {role.description && (
                        <p className="mb-3 text-sm text-muted-foreground">{role.description}</p>
                      )}
                      <PermTable perms={perms} />
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
