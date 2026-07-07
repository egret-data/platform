import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Trash2 } from "lucide-react";

import {
  type AccessRole,
  type PermissionCatalogGroup,
  createRole,
  deleteRole,
  listPermissions,
  listRoles,
  setRolePermissions,
  updateRole,
} from "@/api/access";
import { ApiError } from "@/api/client";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";

export const rolesQueryKey = ["control-plane", "access", "roles"] as const;
export const permissionsQueryKey = ["control-plane", "access", "permissions"] as const;

const COLUMNS = ["Scope", "Name", "Description", "Permissions", "Assignees", ""];

/** Built-in role names, seeded on install — these cannot be deleted. */
const SEEDED_ROLE_NAMES = new Set([
  "platform_admin",
  "data_steward",
  "ws_admin",
  "ws_editor",
  "ws_viewer",
]);

const BUILTIN_DELETE_TOOLTIP = "Built-in role";

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

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => deleteRole(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: rolesQueryKey });
      setDeleteError(null);
    },
    onError: (err) => setDeleteError(err.message ?? "Failed to delete role."),
  });

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
            <Button size="sm" onClick={() => setCreateOpen(true)}>
              <Plus className="mr-1 h-4 w-4" />
              New role
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {deleteError && (
            <Alert variant="destructive" className="mb-4">
              <AlertDescription>{deleteError}</AlertDescription>
            </Alert>
          )}
          <Table>
            <TableHeader>
              <TableRow>
                {COLUMNS.map((col) => (
                  <TableHead
                    key={col || "actions"}
                    className={col === "" ? "text-right" : undefined}
                  >
                    {col || <span className="sr-only">Actions</span>}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((r) => {
                const seeded = isSeeded(r);
                return (
                  <TableRow key={r.id}>
                    <TableCell>
                      <Badge variant={r.scope === "platform" ? "default" : "outline"}>
                        {r.scope}
                      </Badge>
                    </TableCell>
                    <TableCell className="font-medium">
                      {r.name}
                      {seeded && (
                        <Badge variant="secondary" className="ml-2 font-normal">
                          built-in
                        </Badge>
                      )}
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {r.description || "—"}
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{r.permissionCount}</Badge>
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{r.assigneeCount}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      <div className="flex justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`Edit permissions for ${r.name}`}
                          onClick={() => setEditTarget(r)}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`Delete ${r.name}`}
                          disabled={seeded}
                          title={seeded ? BUILTIN_DELETE_TOOLTIP : undefined}
                          onClick={() => {
                            setDeleteError(null);
                            setDeleteTarget(r);
                          }}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
              {rows.length === 0 && (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={COLUMNS.length}
                    className="text-center text-muted-foreground"
                  >
                    {roles.isError ? (
                      <span className="text-destructive">
                        {roles.error?.message ?? "Failed to load roles."}
                      </span>
                    ) : roles.isLoading ? (
                      "Loading…"
                    ) : (
                      "No roles yet."
                    )}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <CreateRoleDialog open={createOpen} onOpenChange={setCreateOpen} />
      <EditRoleDialog
        role={editTarget}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
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
  onOpenChange,
}: {
  role: AccessRole | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const permissions = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: permissionsQueryKey,
    queryFn: listPermissions,
    retry: false,
  });

  const [description, setDescription] = useState("");
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const isSuperuser = role?.name === "platform_admin";
  const groups = permissions.data ?? [];

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

  async function handleSave() {
    if (!role) return;
    setError(null);
    setSaving(true);
    try {
      if (description !== role.description) {
        await updateRole(role.id, description.trim());
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
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
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

          {isSuperuser && (
            <Alert>
              <AlertDescription className="text-muted-foreground">
                Superuser — bypasses all permission checks.
              </AlertDescription>
            </Alert>
          )}

          <div className="space-y-4">
            {permissions.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading…</p>
            ) : permissions.isError ? (
              <p className="text-sm text-destructive">
                {permissions.error?.message ?? "Failed to load permissions."}
              </p>
            ) : (
              groups.map((group) => (
                <div key={group.group} className="space-y-2">
                  <h3 className="text-sm font-semibold">{group.group}</h3>
                  <div className="space-y-2 rounded-md border p-3">
                    {group.permissions.map((p) => {
                      const id = `perm-${p.key}`;
                      return (
                        <div key={p.key} className="flex items-start gap-2">
                          <Checkbox
                            id={id}
                            className="mt-0.5"
                            checked={checked.has(p.key)}
                            disabled={isSuperuser}
                            onCheckedChange={(c) => toggle(p.key, c === true)}
                          />
                          <div className="space-y-0.5">
                            <Label htmlFor={id} className="font-normal">
                              {p.key}
                            </Label>
                            <p className="text-xs text-muted-foreground">
                              {p.description}
                            </p>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))
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
