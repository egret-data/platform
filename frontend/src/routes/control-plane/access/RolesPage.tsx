import { useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Pencil, Plus, Trash2, UserCog } from "lucide-react";

import {
  type AccessRole,
  type RoleAssignee,
  createRole,
  deleteRole,
  getRoleAssignees,
  listRoles,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { RoleImpactNotice } from "@/components/access/RoleImpactNotice";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { EntityLink } from "@/components/ui/entity-link";

export const rolesQueryKey = ["control-plane", "access", "roles"] as const;
export const permissionsQueryKey = ["control-plane", "access", "permissions"] as const;

/** Built-in role names, seeded on install — these cannot be deleted. */
export const SEEDED_ROLE_NAMES = new Set([
  "platform_admin",
  "data_steward",
  "ws_admin",
  "ws_editor",
  "ws_viewer",
]);

const BUILTIN_DELETE_TOOLTIP = "Built-in role";

/**
 * Role ids that must not be offered as `role`'s parent: `role` itself and any
 * role whose parent chain already reaches `role` (choosing one would form an
 * obvious cycle). Computed from the flat roles list.
 */
export function cyclicParentIds(roleId: string, roles: AccessRole[]): Set<string> {
  const parentOf = new Map(roles.map((r) => [r.id, r.parentRoleId ?? null]));
  const banned = new Set<string>([roleId]);
  for (const r of roles) {
    let cur: string | null | undefined = r.id;
    let hops = 0;
    while (cur && hops < 64) {
      if (cur === roleId) {
        banned.add(r.id);
        break;
      }
      cur = parentOf.get(cur) ?? null;
      hops += 1;
    }
  }
  return banned;
}

function isSeeded(role: AccessRole): boolean {
  return SEEDED_ROLE_NAMES.has(role.name);
}

/** Read-only indented tree of the role hierarchy, grouped by scope. */
function RolesTree({
  roles,
  onEdit,
}: {
  roles: AccessRole[];
  onEdit: (role: AccessRole) => void;
}) {
  const childrenOf = useMemo(() => {
    const ids = new Set(roles.map((r) => r.id));
    const m = new Map<string | null, AccessRole[]>();
    for (const r of roles) {
      const parent = r.parentRoleId && ids.has(r.parentRoleId) ? r.parentRoleId : null;
      const arr = m.get(parent) ?? [];
      arr.push(r);
      m.set(parent, arr);
    }
    return m;
  }, [roles]);

  const renderNode = (r: AccessRole, depth: number): ReactNode => {
    if (depth > 64) return null; // cycle guard (DB prevents cycles)
    const kids = childrenOf.get(r.id) ?? [];
    return (
      <div key={r.id}>
        <button
          type="button"
          onClick={() => onEdit(r)}
          title={r.description || undefined}
          className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent"
        >
          <UserCog className="h-4 w-4 shrink-0 text-primary" />
          <span className="min-w-0 truncate font-medium">{r.name}</span>
          {isSeeded(r) && (
            <Badge variant="secondary" className="shrink-0 font-normal">
              built-in
            </Badge>
          )}
          <span className="ml-auto shrink-0 pl-2 text-xs tabular-nums text-muted-foreground">
            {r.permissionCount} perm{r.permissionCount === 1 ? "" : "s"}
            {r.assigneeCount > 0 ? ` · ${r.assigneeCount} assigned` : ""}
          </span>
        </button>
        {kids.length > 0 && (
          // Nested container with a left rail so the hierarchy reads as a tree,
          // not a flat indented list.
          <div className="ml-[15px] border-l pl-2">
            {kids.map((k) => renderNode(k, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  const scopes: ("platform" | "workspace")[] = ["platform", "workspace"];
  const roots = childrenOf.get(null) ?? [];
  return (
    <div className="space-y-4">
      {scopes.map((scope) => {
        const scopeRoots = roots.filter((r) => r.scope === scope);
        if (scopeRoots.length === 0) return null;
        return (
          <div key={scope} className="space-y-1">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              {scope}
            </h3>
            {scopeRoots.map((r) => renderNode(r, 0))}
          </div>
        );
      })}
    </div>
  );
}

/**
 * Control Plane → Access control → Roles.
 *
 * Lists roles (platform + workspace scope) with their permission and
 * assignee counts, and offers create / edit-permissions / delete actions.
 * Built-in (seeded) roles cannot be deleted.
 */
export default function RolesPage() {
  const { me } = useAuth();
  // RBAC-granting writes (create/update/delete role, set permissions) are
  // platform_admin-only on the backend; a role:read-only viewer gets a
  // read-only table with no Actions menu at all.
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const roles = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });
  const rows = roles.data ?? [];
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const openRole = (r: AccessRole) => navigate(`/control-plane/roles/${r.id}`);

  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AccessRole | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [assigneesTarget, setAssigneesTarget] = useState<AccessRole | null>(null);
  const [scopeFacet, setScopeFacet] = useState(FACET_ALL);
  const [treeView, setTreeView] = useState(false);

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => deleteRole(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: rolesQueryKey });
      setDeleteError(null);
    },
    onError: (err) => setDeleteError(err.message ?? "Failed to delete role."),
  });

  const nameById = useMemo(() => new Map(rows.map((r) => [r.id, r.name])), [rows]);

  const columns: Column<AccessRole>[] = [
    {
      // Identity cell: a role glyph + name (primary) + a subtitle that folds in
      // the scope and, when present, the parent it inherits from — replacing the
      // separate Scope / Inherits from / Description columns (description moves
      // to the name's hover title). The Scope facet filter is kept.
      key: "name",
      label: "Role",
      sortable: true,
      value: (r) => r.name,
      cell: (r) => {
        const parentName = r.parentRoleId ? nameById.get(r.parentRoleId) : undefined;
        const scopeLabel = r.scope === "platform" ? "Platform role" : "Workspace role";
        const subtitle = parentName ? `${scopeLabel} · inherits ${parentName}` : scopeLabel;
        return (
          <div className="flex items-center gap-2.5">
            <UserCog className="h-4 w-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <EntityLink
                  to={`/control-plane/roles/${r.id}`}
                  title={r.description || undefined}
                  className="min-w-0 truncate"
                >
                  {r.name}
                </EntityLink>
                {isSeeded(r) && (
                  <Badge variant="secondary" className="shrink-0 font-normal">
                    built-in
                  </Badge>
                )}
              </div>
              <div className="truncate text-xs text-muted-foreground">{subtitle}</div>
            </div>
          </div>
        );
      },
    },
    {
      key: "permissions",
      label: "Permissions",
      cell: (r) => <Badge variant="secondary">{r.permissionCount}</Badge>,
    },
    {
      key: "assignees",
      label: "Assignees",
      cell: (r) =>
        r.assigneeCount > 0 ? (
          <Button
            type="button"
            variant="link"
            className="h-auto p-0"
            aria-label={`View assignees of ${r.name}`}
            onClick={() => setAssigneesTarget(r)}
          >
            <Badge variant="secondary" className="cursor-pointer">
              {r.assigneeCount}
            </Badge>
          </Button>
        ) : (
          <Badge variant="secondary">{r.assigneeCount}</Badge>
        ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (r) => {
        const seeded = isSeeded(r);
        return (
          isAdmin && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="icon" aria-label={`Actions for ${r.name}`}>
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => openRole(r)}>
                  <Pencil className="mr-2 h-4 w-4" />
                  Edit permissions
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  disabled={seeded}
                  title={seeded ? BUILTIN_DELETE_TOOLTIP : undefined}
                  className="text-destructive focus:text-destructive"
                  onClick={() => {
                    setDeleteError(null);
                    setDeleteTarget(r);
                  }}
                >
                  <Trash2 className="mr-2 h-4 w-4" />
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )
        );
      },
    },
  ];

  const filtered = rows.filter(
    (r) => scopeFacet === FACET_ALL || r.scope === scopeFacet,
  );
  const platform = rows.filter((r) => r.scope === "platform").length;

  return (
    <PageContainer className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <PageTitle>Roles</PageTitle>
          <p className="text-sm text-muted-foreground">
            Platform roles apply across all of egret; workspace roles apply inside a single
            workspace and are assigned per-workspace on that workspace's Members page.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant={treeView ? "secondary" : "outline"}
            onClick={() => setTreeView((v) => !v)}
            aria-pressed={treeView}
          >
            {treeView ? "List view" : "Tree view"}
          </Button>
          {isAdmin && (
            <Button size="sm" onClick={() => setCreateOpen(true)}>
              <Plus className="mr-1 h-4 w-4" />
              New role
            </Button>
          )}
        </div>
      </div>

      <div className="space-y-4">
          {deleteError && (
            <Alert variant="destructive" className="mb-4">
              <AlertDescription>{deleteError}</AlertDescription>
            </Alert>
          )}
          <SummaryStrip
            items={[
              { label: "roles", value: rows.length },
              { label: "platform", value: platform },
              { label: "workspace", value: rows.length - platform },
            ]}
          />
          {roles.isLoading ? (
            <ListSkeleton rows={6} cols={7} />
          ) : treeView ? (
            <>
              <p className="text-xs text-muted-foreground">
                Tree view shows the full hierarchy — switch to list to search or filter.
              </p>
              <RolesTree roles={rows} onEdit={openRole} />
            </>
          ) : (
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(r) => r.id}
              searchPlaceholder="Search roles…"
              initialSort={{ key: "name", dir: "asc" }}
              pageSize={25}
              bordered
              emptyText={
                roles.isError ? (
                  (roles.error?.message ?? "Failed to load roles.")
                ) : (
                  <EmptyState
                    icon={UserCog}
                    title="No roles yet"
                    description="Roles bundle permissions, then get assigned to users and groups."
                    action={
                      isAdmin ? (
                        <Button size="sm" onClick={() => setCreateOpen(true)}>
                          <Plus className="mr-1 h-4 w-4" />
                          New role
                        </Button>
                      ) : undefined
                    }
                  />
                )
              }
              filters={
                <FacetSelect
                  label="Scope"
                  value={scopeFacet}
                  onChange={setScopeFacet}
                  options={[
                    { value: "platform", label: "Platform" },
                    { value: "workspace", label: "Workspace" },
                  ]}
                />
              }
            />
          )}
      </div>

      <CreateRoleDialog open={createOpen} onOpenChange={setCreateOpen} />
      <RoleAssigneesDialog
        role={assigneesTarget}
        onOpenChange={(o) => {
          if (!o) setAssigneesTarget(null);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete role?"}
        description="This role will be permanently removed. Users and groups assigned this role will lose the permissions it grants."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
      >
        {deleteTarget && <RoleImpactNotice role={deleteTarget} roles={rows} />}
      </ConfirmDeleteDialog>
    </PageContainer>
  );
}

function CreateRoleDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [scope, setScope] = useState("platform");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createMutation = useMutation<
    AccessRole,
    ApiError,
    { scope: string; name: string; description: string }
  >({
    mutationFn: (input) => createRole(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: rolesQueryKey }),
  });

  function reset() {
    setScope("platform");
    setName("");
    setDescription("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createMutation.mutateAsync({
        scope,
        name: name.trim(),
        description: description.trim(),
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create role.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New role</DialogTitle>
            <DialogDescription>
              Create a role, then edit its permissions afterwards.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="new-role-scope">Scope</Label>
              <Select value={scope} onValueChange={setScope}>
                <SelectTrigger id="new-role-scope" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="platform">Platform</SelectItem>
                  <SelectItem value="workspace">Workspace</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center gap-0.5">
                <Label htmlFor="new-role-name">Name</Label>
                <span className="text-destructive" aria-hidden>
                  *
                </span>
              </div>
              <Input
                id="new-role-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-role-description">Description</Label>
              <Textarea
                id="new-role-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!name.trim() || createMutation.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Read-only drill-in listing who holds a role — users and groups, fetched
 * from `GET /access/roles/{id}/assignees` only while the dialog is open.
 */
function RoleAssigneesDialog({
  role,
  onOpenChange,
}: {
  role: AccessRole | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = role !== null;
  const assignees = useQuery<RoleAssignee[], ApiError>({
    queryKey: ["control-plane", "access", "role-assignees", role?.id ?? ""],
    queryFn: () => getRoleAssignees(role!.id),
    enabled: open,
    retry: false,
  });
  const rows = assignees.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assignees — {role?.name}</DialogTitle>
          <DialogDescription>
            Users and groups currently assigned this role.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-2">
          {assignees.isLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : assignees.isError ? (
            <p className="text-sm text-destructive">
              {assignees.error?.message ?? "Failed to load assignees."}
            </p>
          ) : rows.length === 0 ? (
            <p className="text-sm text-muted-foreground">No assignees.</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              {rows.map((a, i) => (
                <div key={`${a.kind}-${a.name}-${i}`} className="flex items-center gap-1.5">
                  <Badge variant={a.kind === "user" ? "outline" : "secondary"}>
                    {a.kind}
                  </Badge>
                  <span className="text-sm">{a.name}</span>
                </div>
              ))}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
