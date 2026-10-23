import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ChevronsUpDown, Plus, Trash2, X } from "lucide-react";

import {
  type AccessGroup,
  type AccessRole,
  type AccessUser,
  type GroupMember,
  addMember,
  assignGroupRole,
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
import { RolesEditor } from "@/components/access/RolesEditor";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { groupsQueryKey } from "./GroupsPage";
import { usersQueryKey } from "./UsersPage";
import { rolesQueryKey } from "./RolesPage";
import { PageContainer } from "@/components/layout/PageContainer";
import { BackLink, DetailHeader } from "@/components/layout/DetailHeader";

const membersQueryKey = (id: string) =>
  ["control-plane", "access", "group", id, "members"] as const;

/**
 * Control Plane → Access control → Groups → a single group.
 *
 * A deep-linkable detail page mirroring the role/user pages: identity + name,
 * a members table (search a user → Add), and a role-assignment table (browse
 * scope → role → Add). OIDC-synced groups are read-only for name/membership.
 */
export default function GroupDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { me } = useAuth();
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const groupsQ = useQuery<AccessGroup[], ApiError>({
    queryKey: groupsQueryKey,
    queryFn: listGroups,
    retry: false,
  });
  const rolesQ = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });
  const usersQ = useQuery<AccessUser[], ApiError>({
    queryKey: usersQueryKey,
    queryFn: listUsers,
    retry: false,
  });
  const membersQ = useQuery<GroupMember[], ApiError>({
    queryKey: membersQueryKey(id),
    queryFn: () => getGroupMembers(id),
    enabled: !!id,
    retry: false,
  });

  const group = groupsQ.data?.find((g) => g.id === id);
  const allRoles = rolesQ.data ?? [];
  const allUsers = usersQ.data ?? [];
  const members = useMemo(() => membersQ.data ?? [], [membersQ.data]);
  const isOidc = group?.source === "oidc";
  const canEditMembers = isAdmin && !isOidc;

  const [name, setName] = useState("");
  useEffect(() => {
    if (group) setName(group.name);
  }, [group]);

  const [pendingMember, setPendingMember] = useState("");
  const [deleteOpen, setDeleteOpen] = useState(false);

  const invalidateGroups = () => qc.invalidateQueries({ queryKey: groupsQueryKey });
  const invalidateMembers = () => qc.invalidateQueries({ queryKey: membersQueryKey(id) });

  const renameMut = useMutation<AccessGroup, ApiError, string>({
    mutationFn: (n) => renameGroup(id, n),
    onSuccess: () => {
      invalidateGroups();
      toast.success("Group renamed");
    },
    onError: (e) => toast.error(e.message ?? "Failed to rename group."),
  });
  const addMemberMut = useMutation<void, ApiError, string>({
    mutationFn: (userId) => addMember(id, userId),
    onSuccess: () => {
      invalidateMembers();
      invalidateGroups();
      toast.success("Member added");
    },
    onError: (e) => toast.error(e.message ?? "Failed to add member."),
  });
  const removeMemberMut = useMutation<void, ApiError, string>({
    mutationFn: (userId) => removeMember(id, userId),
    onSuccess: () => {
      invalidateMembers();
      invalidateGroups();
      toast.success("Member removed");
    },
    onError: (e) => toast.error(e.message ?? "Failed to remove member."),
  });
  const assignMut = useMutation<void, ApiError, string>({
    mutationFn: (roleId) => assignGroupRole(id, roleId),
    onSuccess: () => {
      invalidateGroups();
      toast.success("Role assigned");
    },
    onError: (e) => toast.error(e.message ?? "Failed to assign role."),
  });
  const unassignMut = useMutation<void, ApiError, string>({
    mutationFn: (roleId) => unassignGroupRole(id, roleId),
    onSuccess: () => {
      invalidateGroups();
      toast.success("Role removed");
    },
    onError: (e) => toast.error(e.message ?? "Failed to remove role."),
  });
  const deleteMut = useMutation<void, ApiError, void>({
    mutationFn: () => deleteGroup(id),
    onSuccess: () => {
      invalidateGroups();
      toast.success("Group deleted");
      navigate("/control-plane/groups");
    },
    onError: (e) => toast.error(e.message ?? "Failed to delete group."),
  });

  const memberIds = useMemo(() => new Set(members.map((m) => m.id)), [members]);

  if (groupsQ.isLoading || rolesQ.isLoading) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/groups">Groups</BackLink>
        <ListSkeleton rows={6} cols={2} />
      </PageContainer>
    );
  }
  if (groupsQ.isError || !group) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/groups">Groups</BackLink>
        <Alert variant="destructive">
          <AlertDescription>
            {groupsQ.error?.message ?? "Group not found."}
          </AlertDescription>
        </Alert>
      </PageContainer>
    );
  }

  return (
    <PageContainer className="space-y-6">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/groups", label: "Groups" }}
        title={group.name}
        titleClassName="min-w-0 truncate"
        description={
          <>
            {isOidc ? "Synced from OIDC" : "Local group"} · {group.memberCount} member
            {group.memberCount === 1 ? "" : "s"}
          </>
        }
        actions={
          isAdmin &&
          !isOidc && (
            <Button
              variant="outline"
              size="sm"
              className="text-destructive hover:text-destructive"
              onClick={() => setDeleteOpen(true)}
            >
              <Trash2 className="mr-1 h-4 w-4" />
              Delete group
            </Button>
          )
        }
      />

      {isOidc && (
        <Alert>
          <AlertDescription className="text-muted-foreground">
            This group is synced from your identity provider — its name and membership are managed
            there and can't be changed here. You can still assign roles to it.
          </AlertDescription>
        </Alert>
      )}

      {/* Details */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold">Details</h2>
        <div className="grid gap-4 sm:max-w-md">
          <div className="space-y-1.5">
            <Label htmlFor="group-name">Name</Label>
            <Input
              id="group-name"
              value={name}
              disabled={!canEditMembers}
              onChange={(e) => setName(e.target.value)}
              onBlur={() => {
                if (canEditMembers && name.trim() && name.trim() !== group.name) {
                  renameMut.mutate(name.trim());
                }
              }}
            />
            {canEditMembers && (
              <p className="text-xs text-muted-foreground">Changes save automatically.</p>
            )}
          </div>
        </div>
      </section>

      {/* Members */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Members</h2>
          <p className="text-sm text-muted-foreground">
            {isOidc
              ? "Members are synced from your identity provider."
              : "Users in this group. Search to add someone, or remove with ×."}
          </p>
        </div>
        {canEditMembers && (
          <div className="flex flex-wrap items-end gap-2">
            <div className="space-y-1">
              <Label className="text-xs text-muted-foreground">User</Label>
              <UserPicker
                users={allUsers}
                excludeIds={memberIds}
                value={pendingMember}
                onChange={setPendingMember}
              />
            </div>
            <Button
              disabled={!pendingMember}
              onClick={() => {
                if (pendingMember) {
                  addMemberMut.mutate(pendingMember);
                  setPendingMember("");
                }
              }}
            >
              <Plus className="mr-1 h-4 w-4" />
              Add
            </Button>
          </div>
        )}
        <div className="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Member</TableHead>
                <TableHead>Email</TableHead>
                <TableHead className="w-10 text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {membersQ.isLoading ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell colSpan={3} className="py-6 text-center text-muted-foreground">
                    Loading…
                  </TableCell>
                </TableRow>
              ) : members.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell colSpan={3} className="py-8 text-center text-muted-foreground">
                    No members yet.
                  </TableCell>
                </TableRow>
              ) : (
                members.map((m) => (
                  <TableRow key={m.id}>
                    <TableCell className="font-medium">{m.name || "—"}</TableCell>
                    <TableCell className="text-muted-foreground">{m.email}</TableCell>
                    <TableCell className="text-right">
                      {canEditMembers ? (
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 text-muted-foreground hover:text-destructive"
                          aria-label={`Remove ${m.email}`}
                          onClick={() => removeMemberMut.mutate(m.id)}
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
      </section>

      {/* Roles */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Roles</h2>
          <p className="text-sm text-muted-foreground">
            Roles granted to every member of this group.
          </p>
        </div>
        <RolesEditor
          assignedRoleNames={group.roles}
          allRoles={allRoles}
          canEdit={isAdmin}
          onAdd={(roleId) => assignMut.mutate(roleId)}
          onRemove={(roleId) => unassignMut.mutate(roleId)}
        />
      </section>

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete "${group.name}"?`}
        description="This group is permanently removed. Its members lose the roles it granted. This cannot be undone."
        confirmText="delete"
        onConfirm={() => {
          deleteMut.mutate();
          setDeleteOpen(false);
        }}
      />
    </PageContainer>
  );
}


/** A searchable single-user picker: search by email/name, click to set the
 * pending selection (the caller confirms with a separate Add button). */
function UserPicker({
  users,
  excludeIds,
  value,
  onChange,
}: {
  users: AccessUser[];
  excludeIds: Set<string>;
  value: string;
  onChange: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const selected = users.find((u) => u.id === value);
  const needle = q.trim().toLowerCase();
  const matches = users
    .filter(
      (u) =>
        !excludeIds.has(u.id) &&
        (needle === "" ||
          u.email.toLowerCase().includes(needle) ||
          u.name.toLowerCase().includes(needle)),
    )
    .slice(0, 50);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-label="User to add"
          className="w-72 justify-between font-normal"
        >
          <span className="truncate">{selected ? selected.email : "Search a user…"}</span>
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-72 p-0" align="start">
        <div className="border-b p-2">
          <Input
            autoFocus
            placeholder="Search by email…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <div className="max-h-64 overflow-y-auto py-1">
          {matches.length === 0 ? (
            <p className="px-3 py-2 text-sm text-muted-foreground">No users found.</p>
          ) : (
            matches.map((u) => (
              <button
                key={u.id}
                type="button"
                className="block w-full px-3 py-1.5 text-left text-sm hover:bg-accent"
                onClick={() => {
                  onChange(u.id);
                  setOpen(false);
                  setQ("");
                }}
              >
                <span className="font-medium">{u.email}</span>
                {u.name ? <span className="text-muted-foreground"> · {u.name}</span> : null}
              </button>
            ))
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
