import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Compass, MoreHorizontal, Pencil, Plus, ShieldCheck, Trash2, UsersRound } from "lucide-react";

import {
  type AccessGroup,
  type AccessRole,
  type AccessUser,
  type GroupMember,
  addMember,
  assignGroupRole,
  createGroup,
  deleteGroup,
  getGroupMembers,
  listGroups,
  listRoles,
  listUsers,
  removeMember,
  renameGroup,
  unassignGroupRole,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
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
import { TagCombobox } from "@/components/ui/tag-combobox";
import { usersQueryKey } from "@/routes/control-plane/access/UsersPage";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { EntityLink } from "@/components/ui/entity-link";

export const groupsQueryKey = ["control-plane", "access", "groups"] as const;
const rolesQueryKey = ["control-plane", "access", "roles"] as const;

function groupMembersQueryKey(groupId: string) {
  return ["control-plane", "access", "group-members", groupId] as const;
}

const OIDC_TOOLTIP = "Synced from your identity provider — read-only here.";

/**
 * Control Plane → Access control → Groups.
 *
 * Lists local + OIDC-synced groups. Local groups can be renamed, deleted, and
 * have their membership managed here; OIDC groups are read-only except for
 * role assignment (their membership is owned by the identity provider).
 */
export default function GroupsPage() {
  const navigate = useNavigate();
  const { me } = useAuth();
  // Assigning roles to a group and managing its membership are
  // platform_admin-only on the backend; group lifecycle (create/rename/
  // delete) stays delegatable.
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const groups = useQuery<AccessGroup[], ApiError>({
    queryKey: groupsQueryKey,
    queryFn: listGroups,
    retry: false,
  });
  const rows = groups.data ?? [];
  const queryClient = useQueryClient();

  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<AccessGroup | null>(null);
  const [membersTarget, setMembersTarget] = useState<AccessGroup | null>(null);
  const [rolesTarget, setRolesTarget] = useState<AccessGroup | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AccessGroup | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [viewMembersTarget, setViewMembersTarget] = useState<AccessGroup | null>(null);
  const [sourceFacet, setSourceFacet] = useState(FACET_ALL);

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => deleteGroup(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: groupsQueryKey });
      setDeleteError(null);
    },
    onError: (err) => setDeleteError(err.message ?? "Failed to delete group."),
  });

  const columns: Column<AccessGroup>[] = [
    {
      // Identity cell: a group glyph + name (primary) with the source folded in
      // as a secondary line ("Synced from OIDC" / "Local group"). OIDC groups are
      // read-only so their name is plain text; a local group's name links to its
      // rename dialog.
      key: "name",
      label: "Group",
      sortable: true,
      value: (g) => g.name,
      cell: (g) => {
        const isOidc = g.source === "oidc";
        return (
          <div className="flex items-center gap-2.5">
            <UsersRound className="h-4 w-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0">
              <EntityLink
                to={`/control-plane/groups/${g.id}`}
                className="block max-w-full truncate"
              >
                {g.name}
              </EntityLink>
              <div className="truncate text-xs text-muted-foreground">
                {isOidc ? "Synced from OIDC" : "Local group"}
              </div>
            </div>
          </div>
        );
      },
    },
    {
      key: "members",
      label: "Members",
      cell: (g) =>
        g.memberCount > 0 ? (
          <Button
            type="button"
            variant="link"
            className="h-auto p-0"
            aria-label={`View members of ${g.name}`}
            onClick={() => setViewMembersTarget(g)}
          >
            {g.memberCount}
          </Button>
        ) : (
          g.memberCount
        ),
    },
    {
      key: "roles",
      label: "Roles",
      // Cap to two chips + a "+N" overflow (rest on hover), matching Users.
      cell: (g) =>
        g.roles.length === 0 ? (
          <span className="text-muted-foreground">—</span>
        ) : (
          <div className="flex flex-wrap items-center gap-1">
            {g.roles.slice(0, 2).map((r) => (
              <Badge key={r} variant="secondary" className="max-w-[10rem] truncate" title={r}>
                {r}
              </Badge>
            ))}
            {g.roles.length > 2 ? (
              <Badge variant="outline" title={g.roles.slice(2).join(", ")}>
                +{g.roles.length - 2}
              </Badge>
            ) : null}
          </div>
        ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (g) => {
        const isOidc = g.source === "oidc";
        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={`Actions for ${g.name}`}>
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem
                onClick={() => navigate(`/control-plane/access-explorer?group=${g.id}`)}
              >
                <Compass className="mr-2 h-4 w-4" />
                View access
              </DropdownMenuItem>
              {isAdmin && (
                <DropdownMenuItem onClick={() => setRolesTarget(g)}>
                  <ShieldCheck className="mr-2 h-4 w-4" />
                  Assign roles
                </DropdownMenuItem>
              )}
              {isAdmin && (
                <DropdownMenuItem
                  disabled={isOidc}
                  title={isOidc ? OIDC_TOOLTIP : undefined}
                  onClick={() => setMembersTarget(g)}
                >
                  <UsersRound className="mr-2 h-4 w-4" />
                  Manage members
                </DropdownMenuItem>
              )}
              <DropdownMenuItem
                disabled={isOidc}
                title={isOidc ? OIDC_TOOLTIP : undefined}
                onClick={() => setRenameTarget(g)}
              >
                <Pencil className="mr-2 h-4 w-4" />
                Rename
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                disabled={isOidc}
                title={isOidc ? OIDC_TOOLTIP : undefined}
                className="text-destructive focus:text-destructive"
                onClick={() => {
                  setDeleteError(null);
                  setDeleteTarget(g);
                }}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        );
      },
    },
  ];

  const filtered = rows.filter(
    (g) => sourceFacet === FACET_ALL || g.source === sourceFacet,
  );
  const totalMembers = rows.reduce((sum, g) => sum + g.memberCount, 0);

  return (
    <PageContainer className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <PageTitle>Groups</PageTitle>
          <p className="text-sm text-muted-foreground">
            Local and identity-provider groups, their membership, and role
            assignments.
          </p>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)}>
          <Plus className="mr-1 h-4 w-4" />
          New group
        </Button>
      </div>

      <div className="space-y-4">
          {deleteError && (
            <Alert variant="destructive" className="mb-4">
              <AlertDescription>{deleteError}</AlertDescription>
            </Alert>
          )}
          <SummaryStrip
            items={[
              { label: "groups", value: rows.length },
              { label: "members", value: totalMembers },
            ]}
          />
          {groups.isLoading ? (
            <ListSkeleton rows={6} cols={5} />
          ) : (
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(g) => g.id}
              searchPlaceholder="Search groups…"
              initialSort={{ key: "name", dir: "asc" }}
              pageSize={25}
              bordered
              emptyText={
                groups.isError ? (
                  (groups.error?.message ?? "Failed to load groups.")
                ) : (
                  <EmptyState
                    icon={UsersRound}
                    title="No groups yet"
                    description="Groups bundle role grants — synced from OIDC or created locally."
                    action={
                      <Button size="sm" onClick={() => setCreateOpen(true)}>
                        <Plus className="mr-1 h-4 w-4" />
                        New group
                      </Button>
                    }
                  />
                )
              }
              filters={
                <FacetSelect
                  label="Source"
                  value={sourceFacet}
                  onChange={setSourceFacet}
                  options={[
                    { value: "local", label: "Local" },
                    { value: "oidc", label: "OIDC" },
                  ]}
                />
              }
            />
          )}
      </div>

      <CreateGroupDialog open={createOpen} onOpenChange={setCreateOpen} />
      <RenameGroupDialog
        group={renameTarget}
        onOpenChange={(o) => {
          if (!o) setRenameTarget(null);
        }}
      />
      <ManageMembersDialog
        group={membersTarget}
        onOpenChange={(o) => {
          if (!o) setMembersTarget(null);
        }}
      />
      <AssignGroupRolesDialog
        group={rolesTarget}
        onOpenChange={(o) => {
          if (!o) setRolesTarget(null);
        }}
      />
      <ViewMembersDialog
        group={viewMembersTarget}
        onOpenChange={(o) => {
          if (!o) setViewMembersTarget(null);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete group?"}
        description="This group will be permanently removed. Role grants on the group are also removed; members keep any roles assigned directly to them."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
      />
    </PageContainer>
  );
}

function CreateGroupDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createMutation = useMutation<AccessGroup, ApiError, { name: string }>({
    mutationFn: (input) => createGroup(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: groupsQueryKey }),
  });

  function reset() {
    setName("");
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
      await createMutation.mutateAsync({ name: name.trim() });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create group.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New group</DialogTitle>
            <DialogDescription>Create a local group.</DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <div className="space-y-1.5">
              <div className="flex items-center gap-0.5">
                <Label htmlFor="new-group-name">Name</Label>
                <span className="text-destructive" aria-hidden>
                  *
                </span>
              </div>
              <Input
                id="new-group-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
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

function RenameGroupDialog({
  group,
  onOpenChange,
}: {
  group: AccessGroup | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (group) {
      setName(group.name);
      setError(null);
    }
  }, [group]);

  const renameMutation = useMutation<AccessGroup, ApiError, string>({
    mutationFn: (newName) => renameGroup(group!.id, newName),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: groupsQueryKey }),
  });

  function handleOpenChange(next: boolean) {
    if (!next) {
      setError(null);
      renameMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await renameMutation.mutateAsync(name.trim());
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to rename group.");
    }
  }

  return (
    <Dialog open={group !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename group</DialogTitle>
            <DialogDescription>Change the display name of this group.</DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <div className="space-y-1.5">
              <Label htmlFor="rename-group-name">Name</Label>
              <Input
                id="rename-group-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={renameMutation.isPending}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Membership management for a local group.
 *
 * Prefills the combobox with the group's current members (fetched via
 * `GET /groups/{id}/members`), then diffs the edited selection against that
 * fetched set on Save — mirroring `AssignGroupRolesDialog`'s add/remove diff
 * — so both adding and removing members are supported.
 */
function ManageMembersDialog({
  group,
  onOpenChange,
}: {
  group: AccessGroup | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const users = useQuery<AccessUser[], ApiError>({
    queryKey: usersQueryKey,
    queryFn: listUsers,
    retry: false,
  });
  const members = useQuery<GroupMember[], ApiError>({
    queryKey: groupMembersQueryKey(group?.id ?? ""),
    queryFn: () => getGroupMembers(group!.id),
    enabled: group !== null,
    retry: false,
  });
  const [selected, setSelected] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (group) {
      setError(null);
    } else {
      setSelected([]);
    }
  }, [group]);

  useEffect(() => {
    if (group && members.data) {
      setSelected(members.data.map((m) => m.email));
    }
  }, [group, members.data]);

  function handleOpenChange(next: boolean) {
    if (!next) setError(null);
    onOpenChange(next);
  }

  async function handleSave() {
    if (!group) return;
    setError(null);
    setSaving(true);
    const currentMembers = members.data ?? [];
    // Users not currently in the group are only reachable via the full user
    // list; current members already carry their own id from the fetch.
    const userIdByEmail = new Map((users.data ?? []).map((u) => [u.email, u.id]));
    for (const m of currentMembers) userIdByEmail.set(m.email, m.id);
    const currentEmails = new Set(currentMembers.map((m) => m.email));
    const nextEmails = new Set(selected);
    const toAdd = selected.filter((email) => !currentEmails.has(email));
    const toRemove = currentMembers
      .map((m) => m.email)
      .filter((email) => !nextEmails.has(email));
    let failed = false;
    try {
      for (const email of toAdd) {
        const userId = userIdByEmail.get(email);
        if (userId) await addMember(group.id, userId);
      }
      for (const email of toRemove) {
        const userId = userIdByEmail.get(email);
        if (userId) await removeMember(group.id, userId);
      }
    } catch (err) {
      failed = true;
      setError((err as ApiError).message ?? "Failed to update members.");
    } finally {
      queryClient.invalidateQueries({ queryKey: groupMembersQueryKey(group.id) });
      queryClient.invalidateQueries({ queryKey: groupsQueryKey });
      setSaving(false);
    }
    if (!failed) handleOpenChange(false);
  }

  return (
    <Dialog open={group !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Manage members — {group?.name}</DialogTitle>
          <DialogDescription>
            Add or remove members of this group. This group currently has{" "}
            {group?.memberCount ?? 0} member{group?.memberCount === 1 ? "" : "s"}.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-2">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <TagCombobox
            values={selected}
            onChange={setSelected}
            suggest={(q) =>
              Promise.resolve(
                (users.data ?? [])
                  .filter((u) => u.email.toLowerCase().includes(q.toLowerCase()))
                  .map((u) => ({ value: u.email, label: u.name })),
              )
            }
            queryKey={["control-plane", "access", "users", "suggest"]}
            placeholder="Add member by email…"
            ariaLabel="Add member"
            creatable={false}
          />
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
 * Read-only drill-in listing a group's members (email + name), fetched via
 * `getGroupMembers` only while the dialog is open. Editing membership is
 * handled separately by `ManageMembersDialog`.
 */
function ViewMembersDialog({
  group,
  onOpenChange,
}: {
  group: AccessGroup | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = group !== null;
  const members = useQuery<GroupMember[], ApiError>({
    queryKey: groupMembersQueryKey(group?.id ?? ""),
    queryFn: () => getGroupMembers(group!.id),
    enabled: open,
    retry: false,
  });
  const rows = members.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Members — {group?.name}</DialogTitle>
          <DialogDescription>Current members of this group.</DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-2">
          {members.isLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : members.isError ? (
            <p className="text-sm text-destructive">
              {members.error?.message ?? "Failed to load members."}
            </p>
          ) : rows.length === 0 ? (
            <p className="text-sm text-muted-foreground">No members.</p>
          ) : (
            <ul className="space-y-1.5">
              {rows.map((m) => (
                <li key={m.id} className="flex items-center justify-between gap-2 text-sm">
                  <span className="font-medium">{m.email}</span>
                  <span className="text-muted-foreground">{m.name}</span>
                </li>
              ))}
            </ul>
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

function AssignGroupRolesDialog({
  group,
  onOpenChange,
}: {
  group: AccessGroup | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const roles = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });
  const [selected, setSelected] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (group) {
      setSelected(group.roles);
      setError(null);
    }
  }, [group]);

  function handleOpenChange(next: boolean) {
    if (!next) setError(null);
    onOpenChange(next);
  }

  async function handleSave() {
    if (!group) return;
    setError(null);
    setSaving(true);
    const roleByName = new Map((roles.data ?? []).map((r) => [r.name, r]));
    const current = new Set(group.roles);
    const next = new Set(selected);
    const toAdd = selected.filter((r) => !current.has(r));
    const toRemove = group.roles.filter((r) => !next.has(r));
    let failed = false;
    try {
      for (const name of toAdd) {
        const role = roleByName.get(name);
        if (role) await assignGroupRole(group.id, role.id);
      }
      for (const name of toRemove) {
        const role = roleByName.get(name);
        if (role) await unassignGroupRole(group.id, role.id);
      }
    } catch (err) {
      failed = true;
      setError((err as ApiError).message ?? "Failed to update roles.");
    } finally {
      queryClient.invalidateQueries({ queryKey: groupsQueryKey });
      queryClient.invalidateQueries({ queryKey: usersQueryKey });
      setSaving(false);
    }
    if (!failed) handleOpenChange(false);
  }

  return (
    <Dialog open={group !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assign roles — {group?.name}</DialogTitle>
          <DialogDescription>
            Roles granted here apply to every member of the group.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-4">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          <TagCombobox
            values={selected}
            onChange={setSelected}
            suggest={(q) =>
              Promise.resolve(
                (roles.data ?? [])
                  .filter((r) => r.name.toLowerCase().includes(q.toLowerCase()))
                  .map((r) => ({ value: r.name, hint: r.scope })),
              )
            }
            queryKey={["control-plane", "access", "roles", "suggest"]}
            placeholder="Add role…"
            ariaLabel="Add role"
            creatable={false}
          />
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
