import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Plus, Trash2, X } from "lucide-react";

import {
  type AccessRole,
  type PermissionCatalogGroup,
  deleteRole,
  getRole,
  listPermissions,
  listRoles,
  setRolePermissions,
  updateRole,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { RoleImpactNotice } from "@/components/access/RoleImpactNotice";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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

import {
  SEEDED_ROLE_NAMES,
  cyclicParentIds,
  permissionsQueryKey,
  rolesQueryKey,
} from "./RolesPage";
import { PageContainer } from "@/components/layout/PageContainer";
import { BackLink, DetailHeader } from "@/components/layout/DetailHeader";

const NO_PARENT = "__none__";
const roleQueryKey = (id: string) => ["control-plane", "access", "role", id] as const;

/**
 * Control Plane → Access control → Roles → a single role.
 *
 * A dedicated detail page (deep-linkable) that replaces the old edit-in-a-modal
 * flow. The permissions the role holds are shown as a plain table; new ones are
 * added by browsing (category → permission) and confirming with an Add button.
 * Removing a permission and editing the description/inheritance auto-save (a
 * permission set is a full-set PUT).
 */
export default function RoleDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { me } = useAuth();
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const rolesQ = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });
  const roleQ = useQuery<AccessRole, ApiError>({
    queryKey: roleQueryKey(id),
    queryFn: () => getRole(id),
    enabled: !!id,
    retry: false,
  });
  const permsQ = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: permissionsQueryKey,
    queryFn: listPermissions,
    retry: false,
  });

  const roles = rolesQ.data ?? [];
  const role = roleQ.data;
  const groups = useMemo(() => permsQ.data ?? [], [permsQ.data]);
  const isSuperuser = role?.name === "platform_admin";

  const inherited = useMemo(
    () => new Set(role?.inheritedPermissions ?? []),
    [role],
  );

  const initialGranted = useMemo(() => {
    const keys = new Set<string>();
    if (!role) return keys;
    for (const g of groups) {
      for (const p of g.permissions) {
        if (p.roles.includes(role.name)) keys.add(p.key);
      }
    }
    return keys;
  }, [role, groups]);

  const [granted, setGranted] = useState<Set<string>>(new Set());
  useEffect(() => setGranted(initialGranted), [initialGranted]);

  const [description, setDescription] = useState("");
  const [parent, setParent] = useState<string>(NO_PARENT);
  useEffect(() => {
    if (role) {
      setDescription(role.description);
      setParent(role.parentRoleId ?? NO_PARENT);
    }
  }, [role]);

  const [deleteOpen, setDeleteOpen] = useState(false);

  const setPermsMut = useMutation<void, ApiError, string[]>({
    mutationFn: (keys) => setRolePermissions(id, keys),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: permissionsQueryKey });
      qc.invalidateQueries({ queryKey: rolesQueryKey });
    },
  });

  const updateMut = useMutation<AccessRole, ApiError, { description: string; parent: string | null }>({
    mutationFn: (v) => updateRole(id, v.description, v.parent),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: rolesQueryKey });
      qc.invalidateQueries({ queryKey: roleQueryKey(id) });
    },
  });

  const deleteMut = useMutation<void, ApiError, void>({
    mutationFn: () => deleteRole(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: rolesQueryKey });
      toast.success("Role deleted");
      navigate("/control-plane/roles");
    },
    onError: (e) => toast.error(e.message ?? "Failed to delete role."),
  });

  /** Persist a new granted set, rolling the optimistic state back on failure. */
  function persistGranted(next: Set<string>, verb: string) {
    const prev = granted;
    setGranted(next);
    setPermsMut.mutate(Array.from(next), {
      onSuccess: () => toast.success(verb),
      onError: (e) => {
        setGranted(prev);
        toast.error(e.message ?? "Failed to update permissions.");
      },
    });
  }

  function removePerm(key: string) {
    const next = new Set(granted);
    next.delete(key);
    persistGranted(next, "Permission removed");
  }

  function addPerm(key: string) {
    const next = new Set(granted);
    next.add(key);
    persistGranted(next, "Permission added");
  }

  function saveDetails(nextDescription: string, nextParent: string) {
    const parentId = nextParent === NO_PARENT ? null : nextParent;
    if (
      role &&
      nextDescription.trim() === role.description &&
      parentId === (role.parentRoleId ?? null)
    ) {
      return; // nothing changed
    }
    updateMut.mutate(
      { description: nextDescription.trim(), parent: parentId },
      {
        onSuccess: () => toast.success("Role updated"),
        onError: (e) => toast.error(e.message ?? "Failed to update role."),
      },
    );
  }

  const parentOptions = useMemo(() => {
    if (!role) return [] as AccessRole[];
    const banned = cyclicParentIds(role.id, roles);
    return roles.filter((r) => r.scope === role.scope && !banned.has(r.id));
  }, [role, roles]);

  // Flat rows for the permissions table: the granted + inherited permissions the
  // role holds, in catalogue order, each tagged with its category and source.
  const permRows = useMemo(() => {
    if (!role) return [] as { key: string; description: string; category: string; inherited: boolean }[];
    return groups.flatMap((g) =>
      g.permissions
        .filter((p) => p.scope === role.scope && (granted.has(p.key) || inherited.has(p.key)))
        .map((p) => ({
          key: p.key,
          description: p.description,
          category: g.group,
          inherited: inherited.has(p.key) && !granted.has(p.key),
        })),
    );
  }, [groups, role, granted, inherited]);

  // Not-yet-granted permissions, grouped, for the browse-by-category add.
  const availableGroups = useMemo(() => {
    if (!role) return [];
    return groups
      .map((g) => ({
        group: g.group,
        permissions: g.permissions.filter(
          (p) => p.scope === role.scope && !granted.has(p.key) && !inherited.has(p.key),
        ),
      }))
      .filter((g) => g.permissions.length > 0);
  }, [groups, role, granted, inherited]);

  const [addGroup, setAddGroup] = useState("");
  const [pendingPerm, setPendingPerm] = useState("");
  useEffect(() => {
    if (addGroup && !availableGroups.some((g) => g.group === addGroup)) setAddGroup("");
  }, [availableGroups, addGroup]);
  useEffect(() => setPendingPerm(""), [addGroup]);
  const addablePerms = availableGroups.find((g) => g.group === addGroup)?.permissions ?? [];

  const heldCount = permRows.length;

  if (roleQ.isLoading || rolesQ.isLoading) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/roles">Roles</BackLink>
        <ListSkeleton rows={6} cols={2} />
      </PageContainer>
    );
  }
  if (roleQ.isError || !role) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/roles">Roles</BackLink>
        <Alert variant="destructive">
          <AlertDescription>
            {roleQ.error?.message ?? "Role not found."}
          </AlertDescription>
        </Alert>
      </PageContainer>
    );
  }

  const seeded = SEEDED_ROLE_NAMES.has(role.name);
  const canEdit = isAdmin && !isSuperuser;

  return (
    <PageContainer className="space-y-6">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/roles", label: "Roles" }}
        title={role.name}
        titleClassName="min-w-0 truncate"
        status={
          seeded && (
            <Badge variant="secondary" className="font-normal">
              built-in
            </Badge>
          )
        }
        description={
          <>
            {role.scope === "platform" ? "Platform role" : "Workspace role"} · {heldCount}{" "}
            permission{heldCount === 1 ? "" : "s"} · {role.assigneeCount} assignee
            {role.assigneeCount === 1 ? "" : "s"}
          </>
        }
        actions={
          isAdmin &&
          !seeded && (
            <Button
              variant="outline"
              size="sm"
              className="text-destructive hover:text-destructive"
              onClick={() => setDeleteOpen(true)}
            >
              <Trash2 className="mr-1 h-4 w-4" />
              Delete role
            </Button>
          )
        }
      />

      <RoleImpactNotice role={role} roles={roles} />

      {/* Details */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold">Details</h2>
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="role-description">Description</Label>
            <Textarea
              id="role-description"
              className="min-h-16"
              value={description}
              disabled={!isAdmin}
              placeholder={isAdmin ? "What is this role for?" : undefined}
              onChange={(e) => setDescription(e.target.value)}
              onBlur={() => saveDetails(description, parent)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="role-parent">Inherits from</Label>
            <Select
              value={parent}
              disabled={!canEdit}
              onValueChange={(v) => {
                setParent(v);
                saveDetails(description, v);
              }}
            >
              <SelectTrigger id="role-parent" aria-label="Inherits from" className="w-full">
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
              Also grants everything its parent grants. Changes save automatically.
            </p>
          </div>
        </div>
      </section>

      {/* Permissions */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Permissions</h2>
          <p className="text-sm text-muted-foreground">
            What this {role.scope} role grants. Inherited permissions come from its parent and
            can't be removed here.
          </p>
        </div>

        {isSuperuser ? (
          <Alert>
            <AlertDescription className="text-muted-foreground">
              <span className="font-medium text-foreground">Superuser</span> — this role bypasses
              all permission checks.
            </AlertDescription>
          </Alert>
        ) : (
          <>
            {/* Browse-by-category add: choose a category, then a permission, then Add. */}
            {canEdit && (
              <div className="flex flex-wrap items-end gap-2">
                <div className="space-y-1">
                  <Label className="text-xs text-muted-foreground">Category</Label>
                  <Select
                    value={addGroup}
                    onValueChange={setAddGroup}
                    disabled={availableGroups.length === 0}
                  >
                    <SelectTrigger className="w-44" aria-label="Permission category">
                      <SelectValue placeholder="Choose a category" />
                    </SelectTrigger>
                    <SelectContent>
                      {availableGroups.map((g) => (
                        <SelectItem key={g.group} value={g.group}>
                          {g.group}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs text-muted-foreground">Permission</Label>
                  <Select value={pendingPerm} onValueChange={setPendingPerm} disabled={!addGroup}>
                    <SelectTrigger className="w-72" aria-label="Permission to add">
                      <SelectValue
                        placeholder={addGroup ? "Select a permission" : "Choose a category first"}
                      />
                    </SelectTrigger>
                    <SelectContent>
                      {addablePerms.map((p) => (
                        <SelectItem key={p.key} value={p.key}>
                          <span className="font-medium">{p.key}</span>
                          {p.description ? (
                            <span className="text-muted-foreground"> — {p.description}</span>
                          ) : null}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <Button
                  disabled={!pendingPerm}
                  onClick={() => {
                    if (pendingPerm) {
                      addPerm(pendingPerm);
                      setPendingPerm("");
                    }
                  }}
                >
                  <Plus className="mr-1 h-4 w-4" />
                  Add
                </Button>
                {availableGroups.length === 0 && (
                  <p className="pb-2 text-xs text-muted-foreground">
                    All available permissions are already granted.
                  </p>
                )}
              </div>
            )}

            <div className="rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Permission</TableHead>
                    <TableHead>Category</TableHead>
                    <TableHead>Description</TableHead>
                    <TableHead>Source</TableHead>
                    <TableHead className="w-10 text-right">
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {permsQ.isLoading ? (
                    <TableRow className="hover:bg-transparent">
                      <TableCell colSpan={5} className="py-6 text-center text-muted-foreground">
                        Loading…
                      </TableCell>
                    </TableRow>
                  ) : permRows.length === 0 ? (
                    <TableRow className="hover:bg-transparent">
                      <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                        No permissions granted yet.
                        {canEdit ? " Pick a category above to grant some." : ""}
                      </TableCell>
                    </TableRow>
                  ) : (
                    permRows.map((p) => (
                      <TableRow key={p.key}>
                        <TableCell className="font-medium">{p.key}</TableCell>
                        <TableCell className="text-muted-foreground">{p.category}</TableCell>
                        <TableCell className="text-muted-foreground">
                          {p.description || "—"}
                        </TableCell>
                        <TableCell>
                          {p.inherited ? (
                            <Badge variant="secondary" className="font-normal">
                              inherited
                            </Badge>
                          ) : (
                            <Badge variant="outline" className="font-normal">
                              direct
                            </Badge>
                          )}
                        </TableCell>
                        <TableCell className="text-right">
                          {!p.inherited && canEdit ? (
                            <Button
                              variant="ghost"
                              size="icon"
                              className="h-7 w-7 text-muted-foreground hover:text-destructive"
                              aria-label={`Remove ${p.key}`}
                              onClick={() => removePerm(p.key)}
                            >
                              <X className="h-4 w-4" />
                            </Button>
                          ) : null}
                        </TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>
          </>
        )}
      </section>

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete "${role.name}"?`}
        description="This role and its permission grants are permanently removed. Users and groups assigned to it lose these permissions. This cannot be undone."
        confirmText="delete"
        onConfirm={() => {
          deleteMut.mutate();
          setDeleteOpen(false);
        }}
      />
    </PageContainer>
  );
}

