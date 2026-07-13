import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Pencil, Plus, Search, Trash2 } from "lucide-react";

import {
  type AccessRole,
  type PermissionCatalogGroup,
  type RoleAssignee,
  createRole,
  deleteRole,
  getRole,
  getRoleAssignees,
  listPermissions,
  listRoles,
  setRolePermissions,
  updateRole,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
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

export const rolesQueryKey = ["control-plane", "access", "roles"] as const;
export const permissionsQueryKey = ["control-plane", "access", "permissions"] as const;

/** Built-in role names, seeded on install — these cannot be deleted. */
const SEEDED_ROLE_NAMES = new Set([
  "platform_admin",
  "data_steward",
  "ws_admin",
  "ws_editor",
  "ws_viewer",
]);

const BUILTIN_DELETE_TOOLTIP = "Built-in role";

/** Sentinel Select value for "no parent" (a role at the top of a chain). */
const NO_PARENT = "__none__";

/**
 * Role ids that must not be offered as `role`'s parent: `role` itself and any
 * role whose parent chain already reaches `role` (choosing one would form an
 * obvious cycle). Computed from the flat roles list.
 */
function cyclicParentIds(roleId: string, roles: AccessRole[]): Set<string> {
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

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<AccessRole | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AccessRole | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [assigneesTarget, setAssigneesTarget] = useState<AccessRole | null>(null);
  const [scopeFacet, setScopeFacet] = useState(FACET_ALL);

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => deleteRole(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: rolesQueryKey });
      setDeleteError(null);
    },
    onError: (err) => setDeleteError(err.message ?? "Failed to delete role."),
  });

  const columns: Column<AccessRole>[] = [
    {
      key: "scope",
      label: "Scope",
      cell: (r) => (
        <Badge variant={r.scope === "platform" ? "default" : "outline"}>{r.scope}</Badge>
      ),
    },
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (r) => r.name,
      cell: (r) => (
        <>
          {r.name}
          {isSeeded(r) && (
            <Badge variant="secondary" className="ml-2 font-normal">
              built-in
            </Badge>
          )}
        </>
      ),
    },
    {
      key: "description",
      label: "Description",
      value: (r) => r.description,
      cell: (r) => (
        <span className="text-muted-foreground">{r.description || "—"}</span>
      ),
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
                <DropdownMenuItem onClick={() => setEditTarget(r)}>
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
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Roles</h1>
        <p className="text-sm text-muted-foreground">
          Platform and workspace roles, their permission grants, and how many
          users/groups are assigned.
        </p>
      </div>

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between gap-2">
            <div className="space-y-1.5">
              <CardTitle className="text-lg">All roles</CardTitle>
              <CardDescription>
                Edit a role's permissions to change what it grants.
              </CardDescription>
            </div>
            {isAdmin && (
              <Button size="sm" onClick={() => setCreateOpen(true)}>
                <Plus className="mr-1 h-4 w-4" />
                New role
              </Button>
            )}
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
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
          <FilterableTable
            rows={filtered}
            columns={columns}
            getId={(r) => r.id}
            searchPlaceholder="Search roles…"
            initialSort={{ key: "name", dir: "asc" }}
            emptyText={
              roles.isLoading
                ? "Loading…"
                : roles.isError
                  ? (roles.error?.message ?? "Failed to load roles.")
                  : "No roles."
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
        </CardContent>
      </Card>

      <CreateRoleDialog open={createOpen} onOpenChange={setCreateOpen} />
      <EditRoleDialog
        role={editTarget}
        roles={rows}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
      />
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
      />
    </div>
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
              <Label htmlFor="new-role-name">Name</Label>
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
            <Button type="submit" disabled={createMutation.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Edit-permissions dialog for a single role.
 *
 * `AccessRole` doesn't carry the role's current permission keys directly
 * (only a `permissionCount`) — there is no "get one role's permissions"
 * endpoint. Instead this derives the checked set from `listPermissions`:
 * a key is checked iff this role's name appears in that permission's
 * `roles` array. `platform_admin` is a superuser that bypasses permission
 * checks entirely, so its checklist is shown for reference only, disabled.
 */
function EditRoleDialog({
  role,
  roles,
  onOpenChange,
}: {
  role: AccessRole | null;
  roles: AccessRole[];
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const permissions = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: permissionsQueryKey,
    queryFn: listPermissions,
    retry: false,
  });

  // The list endpoint returns an empty `inheritedPermissions`; the single-role
  // GET populates it (and the current parent), so fetch it while open.
  const detail = useQuery<AccessRole, ApiError>({
    queryKey: ["control-plane", "access", "role", role?.id ?? ""],
    queryFn: () => getRole(role!.id),
    enabled: role !== null,
    retry: false,
  });

  const [description, setDescription] = useState("");
  const [parent, setParent] = useState<string>(NO_PARENT);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const isSuperuser = role?.name === "platform_admin";
  const groups = permissions.data ?? [];

  const [permQuery, setPermQuery] = useState("");

  // Only permissions this role's scope can hold; then apply the in-dialog search.
  const shownGroups = useMemo(() => {
    const needle = permQuery.trim().toLowerCase();
    return groups
      .map((g) => ({
        ...g,
        permissions: g.permissions.filter(
          (p) =>
            (!role || p.scope === role.scope) &&
            (needle === "" ||
              p.key.toLowerCase().includes(needle) ||
              p.description.toLowerCase().includes(needle)),
        ),
      }))
      .filter((g) => g.permissions.length > 0);
  }, [groups, role, permQuery]);

  // Roles offered as a parent: same scope, excluding self and any role whose
  // chain already reaches this one (obvious cycle).
  const parentOptions = useMemo(() => {
    if (!role) return [] as AccessRole[];
    const banned = cyclicParentIds(role.id, roles);
    return roles.filter((r) => r.scope === role.scope && !banned.has(r.id));
  }, [role, roles]);

  const inherited = useMemo(
    () => new Set(detail.data?.inheritedPermissions ?? []),
    [detail.data],
  );

  const initialChecked = useMemo(() => {
    if (!role) return new Set<string>();
    const keys = new Set<string>();
    for (const group of groups) {
      for (const p of group.permissions) {
        if (p.roles.includes(role.name)) keys.add(p.key);
      }
    }
    return keys;
  }, [role, groups]);

  useEffect(() => {
    if (role) {
      setDescription(role.description);
      setParent(role.parentRoleId ?? NO_PARENT);
      setChecked(initialChecked);
      setError(null);
    }
  }, [role, initialChecked]);

  function handleOpenChange(next: boolean) {
    if (!next) setError(null);
    onOpenChange(next);
  }

  function toggle(key: string, isChecked: boolean) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (isChecked) next.add(key);
      else next.delete(key);
      return next;
    });
  }

  /** Toggle every non-inherited permission of a group on/off at once. */
  function toggleGroup(perms: { key: string }[], on: boolean) {
    setChecked((prev) => {
      const next = new Set(prev);
      for (const p of perms) {
        if (inherited.has(p.key)) continue; // inherited perms are fixed
        if (on) next.add(p.key);
        else next.delete(p.key);
      }
      return next;
    });
  }

  async function handleSave() {
    if (!role) return;
    setError(null);
    setSaving(true);
    try {
      const nextParent = parent === NO_PARENT ? null : parent;
      const parentChanged = nextParent !== (role.parentRoleId ?? null);
      if (description !== role.description || parentChanged) {
        await updateRole(role.id, description.trim(), nextParent);
      }
      if (!isSuperuser) {
        await setRolePermissions(role.id, Array.from(checked));
      }
      queryClient.invalidateQueries({ queryKey: rolesQueryKey });
      queryClient.invalidateQueries({ queryKey: permissionsQueryKey });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save role.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={role !== null} onOpenChange={handleOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-3xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Edit role — {role?.name}</DialogTitle>
          <DialogDescription>
            Update the description and choose which permissions this role
            grants.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-4">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          <div className="space-y-1.5">
            <Label htmlFor="edit-role-description">Description</Label>
            <Textarea
              id="edit-role-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="edit-role-parent">Inherits from</Label>
            <Select value={parent} onValueChange={setParent} disabled={isSuperuser}>
              <SelectTrigger id="edit-role-parent" aria-label="Inherits from" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NO_PARENT}>No parent</SelectItem>
                {parentOptions.map((r) => (
                  <SelectItem key={r.id} value={r.id}>
                    {r.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              This role also grants everything its parent grants.
            </p>
          </div>

          {isSuperuser && (
            <Alert>
              <AlertDescription className="text-muted-foreground">
                Superuser — bypasses all permission checks.
              </AlertDescription>
            </Alert>
          )}

          {!isSuperuser && role && (
            <p className="text-xs text-muted-foreground">
              This is a <span className="font-medium text-foreground">{role.scope}</span> role
              — it can hold {role.scope}-scoped permissions.
            </p>
          )}

          {!isSuperuser && !permissions.isLoading && !permissions.isError && groups.length > 0 && (
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder="Search permissions…"
                value={permQuery}
                onChange={(e) => setPermQuery(e.target.value)}
              />
            </div>
          )}

          <div className="space-y-4">
            {permissions.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading…</p>
            ) : permissions.isError ? (
              <p className="text-sm text-destructive">
                {permissions.error?.message ?? "Failed to load permissions."}
              </p>
            ) : shownGroups.length === 0 ? (
              <p className="text-sm text-muted-foreground">No permissions match your search.</p>
            ) : (
              shownGroups.map((group) => {
                const editable = group.permissions.filter((p) => !inherited.has(p.key));
                const allOn = editable.length > 0 && editable.every((p) => checked.has(p.key));
                return (
                  <div key={group.group} className="space-y-1.5">
                    <div className="flex items-center gap-2">
                      {!isSuperuser && editable.length > 0 && (
                        <Checkbox
                          aria-label={`Select all ${group.group}`}
                          checked={allOn}
                          onCheckedChange={(c) => toggleGroup(editable, c === true)}
                        />
                      )}
                      <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                        {group.group}
                      </h3>
                    </div>
                    <div className="grid grid-cols-1 gap-x-6 gap-y-1.5 rounded-md border p-3 sm:grid-cols-2">
                      {group.permissions.map((p) => {
                        const id = `perm-${p.key}`;
                        const isInherited = inherited.has(p.key);
                        return (
                          <div key={p.key} className="flex items-center gap-2">
                            <Checkbox
                              id={id}
                              checked={isInherited || checked.has(p.key)}
                              disabled={isSuperuser || isInherited}
                              onCheckedChange={(c) => toggle(p.key, c === true)}
                            />
                            <Label htmlFor={id} className="cursor-pointer font-normal" title={p.description}>
                              {p.key}
                            </Label>
                            {isInherited && (
                              <Badge variant="secondary" className="font-normal">
                                inherited
                              </Badge>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={handleSave} disabled={saving}>
            Save
          </Button>
        </DialogFooter>
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
