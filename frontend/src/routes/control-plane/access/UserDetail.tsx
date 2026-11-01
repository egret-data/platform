import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { KeyRound, Trash2 } from "lucide-react";

import {
  type AccessRole,
  type AccessUser,
  assignUserRole,
  deleteUser,
  listRoles,
  listUsers,
  resetUserPassword,
  unassignUserRole,
  updateUser,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { RolesEditor } from "@/components/access/RolesEditor";
import { lastActiveAdminReason } from "@/components/access/lastActiveAdmin";
import { DeleteUserDialog, DisableUserDialog } from "@/components/access/UserLifecycleDialogs";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { usersQueryKey } from "./UsersPage";
import { rolesQueryKey } from "./RolesPage";
import { PageContainer } from "@/components/layout/PageContainer";
import { BackLink, DetailHeader } from "@/components/layout/DetailHeader";
import { StatusBadge } from "@/components/ui/status-badge";

/**
 * Control Plane → Access control → Users → a single user.
 *
 * A deep-linkable detail page: identity + status, a role-assignment table
 * (browse scope → role → Add, like the role detail page) and a read-only view
 * of the groups the user belongs to. Editing the name auto-saves; role and
 * status changes persist immediately.
 */
export default function UserDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { me } = useAuth();
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const usersQ = useQuery<AccessUser[], ApiError>({
    queryKey: usersQueryKey,
    queryFn: listUsers,
    retry: false,
  });
  const rolesQ = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });

  const user = usersQ.data?.find((u) => u.id === id);
  const allRoles = rolesQ.data ?? [];

  const [name, setName] = useState("");
  useEffect(() => {
    if (user) setName(user.name);
  }, [user]);

  const [resetOpen, setResetOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [disableOpen, setDisableOpen] = useState(false);

  const invalidate = () => qc.invalidateQueries({ queryKey: usersQueryKey });

  const updateMut = useMutation<AccessUser, ApiError, { name?: string; status?: string }>({
    mutationFn: (input) => updateUser(id, input),
    onSuccess: invalidate,
  });
  const assignMut = useMutation<void, ApiError, string>({
    mutationFn: (roleId) => assignUserRole(id, roleId),
    onSuccess: () => {
      invalidate();
      toast.success("Role assigned");
    },
    onError: (e) => toast.error(e.message ?? "Failed to assign role."),
  });
  const unassignMut = useMutation<void, ApiError, string>({
    mutationFn: (roleId) => unassignUserRole(id, roleId),
    onSuccess: () => {
      invalidate();
      toast.success("Role removed");
    },
    onError: (e) => toast.error(e.message ?? "Failed to remove role."),
  });
  const deleteMut = useMutation<void, ApiError, void>({
    mutationFn: () => deleteUser(id),
    onSuccess: () => {
      invalidate();
      toast.success("User deleted");
      navigate("/control-plane/users");
    },
    onError: (e) => toast.error(e.message ?? "Failed to delete user."),
  });

  function saveName() {
    if (user && name.trim() !== user.name) {
      updateMut.mutate(
        { name: name.trim() },
        {
          onSuccess: () => toast.success("User updated"),
          onError: (e) => toast.error(e.message ?? "Failed to update user."),
        },
      );
    }
  }

  function setStatus(next: "active" | "disabled") {
    updateMut.mutate(
      { status: next },
      {
        onSuccess: () => toast.success(next === "active" ? "User enabled" : "User disabled"),
        onError: (e) => toast.error(e.message ?? "Failed to update status."),
      },
    );
  }

  if (usersQ.isLoading || rolesQ.isLoading) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/users">Users</BackLink>
        <ListSkeleton rows={6} cols={2} />
      </PageContainer>
    );
  }
  if (usersQ.isError || !user) {
    return (
      <PageContainer className="space-y-5">
        <BackLink to="/control-plane/users">Users</BackLink>
        <Alert variant="destructive">
          <AlertDescription>
            {usersQ.error?.message ?? "User not found."}
          </AlertDescription>
        </Alert>
      </PageContainer>
    );
  }

  const isActive = user.status === "active";
  const isSelf = user.id === me?.user_id;
  // KI-260: the roles the user holds through a group, as the server reports
  // them; the editor above lists the directly assigned ones.
  const groupRoles = user.effectiveRoles.filter(
    (r): r is { name: string; group: string } => r.group != null,
  );

  return (
    <PageContainer className="space-y-6">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/users", label: "Users" }}
        title={user.name?.trim() || user.email}
        titleClassName="min-w-0 truncate"
        status={
          <>
            {isActive ? (
              <StatusBadge tone="success">active</StatusBadge>
            ) : (
              <StatusBadge tone="inactive">disabled</StatusBadge>
            )}
            <Badge variant="outline" className="font-normal">
              {user.provider}
            </Badge>
          </>
        }
        description={<span className="block truncate">{user.email}</span>}
        actions={
          isAdmin && (
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={() => (isActive ? setDisableOpen(true) : setStatus("active"))}
              >
                {isActive ? "Disable" : "Enable"}
              </Button>
              {user.provider === "local" && (
                <Button variant="outline" size="sm" onClick={() => setResetOpen(true)}>
                  <KeyRound className="mr-1 h-4 w-4" />
                  Reset password
                </Button>
              )}
              <Button
                variant="outline"
                size="sm"
                className="text-destructive hover:text-destructive"
                disabled={isSelf}
                title={isSelf ? "You cannot delete your own account" : undefined}
                onClick={() => setDeleteOpen(true)}
              >
                <Trash2 className="mr-1 h-4 w-4" />
                Delete
              </Button>
            </>
          )
        }
      />

      {/* Details */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold">Details</h2>
        <div className="grid gap-4 sm:max-w-md">
          <div className="space-y-1.5">
            <Label htmlFor="user-name">Name</Label>
            <Input
              id="user-name"
              value={name}
              disabled={!isAdmin}
              onChange={(e) => setName(e.target.value)}
              onBlur={saveName}
            />
            <p className="text-xs text-muted-foreground">Changes save automatically.</p>
          </div>
        </div>
      </section>

      {/* Roles */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Roles</h2>
          <p className="text-sm text-muted-foreground">
            Platform-wide roles assigned to this user. Workspace roles are assigned per workspace.
          </p>
        </div>
        <RolesEditor
          assignedRoleNames={user.roles}
          allRoles={allRoles}
          canEdit={isAdmin}
          onAdd={(roleId) => assignMut.mutate(roleId)}
          onRemove={(roleId) => unassignMut.mutate(roleId)}
        />
        <div className="space-y-2">
          <h3 className="text-sm font-semibold">Through groups</h3>
          <p className="text-sm text-muted-foreground">
            Platform roles this user holds as a member of a group. Change them on the group's page.
          </p>
          <div className="rounded-md border">
            <Table aria-label="Roles held through groups">
              <TableHeader>
                <TableRow>
                  <TableHead>Role</TableHead>
                  <TableHead>Group</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {groupRoles.length === 0 ? (
                  <TableRow className="hover:bg-transparent">
                    <TableCell colSpan={2} className="py-6 text-center text-muted-foreground">
                      No platform role through a group.
                    </TableCell>
                  </TableRow>
                ) : (
                  groupRoles.map((r) => (
                    <TableRow key={`${r.name}:${r.group}`}>
                      <TableCell className="font-medium">{r.name}</TableCell>
                      <TableCell>{r.group}</TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </div>
      </section>

      {/* Groups (read-only — membership is managed from the Groups page) */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Groups</h2>
          <p className="text-sm text-muted-foreground">
            Groups this user belongs to. Manage membership from the group's page.
          </p>
        </div>
        <div className="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Group</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {user.groups.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell className="py-8 text-center text-muted-foreground">
                    Not a member of any group.
                  </TableCell>
                </TableRow>
              ) : (
                user.groups.map((g) => (
                  <TableRow key={g}>
                    <TableCell className="font-medium">{g}</TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      </section>

      <ResetPasswordDialog userId={id} email={user.email} open={resetOpen} onOpenChange={setResetOpen} />

      <DisableUserDialog
        user={user}
        open={disableOpen}
        onOpenChange={setDisableOpen}
        onConfirm={() => setStatus("disabled")}
      />

      <DeleteUserDialog
        user={user}
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        onConfirm={() => {
          deleteMut.mutate();
          setDeleteOpen(false);
        }}
        onDisableInstead={() => {
          setDeleteOpen(false);
          setDisableOpen(true);
        }}
        canDisable={isAdmin}
        disableBlockedReason={lastActiveAdminReason(user, usersQ.data ?? [])}
      />
    </PageContainer>
  );
}


function ResetPasswordDialog({
  userId,
  email,
  open,
  onOpenChange,
}: {
  userId: string;
  email: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const resetMut = useMutation<void, ApiError, string>({
    mutationFn: (pw) => resetUserPassword(userId, pw),
  });

  useEffect(() => {
    if (!open) {
      setPassword("");
      setShow(false);
    }
  }, [open]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await resetMut.mutateAsync(password);
      toast.success("Password reset");
      onOpenChange(false);
    } catch (err) {
      toast.error((err as ApiError).message ?? "Failed to reset password.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Reset password — {email}</DialogTitle>
            <DialogDescription>
              Set a new password for this local account. Their existing sessions are signed out
              immediately.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-1.5">
            <Label htmlFor="reset-pw">New password</Label>
            <Input
              id="reset-pw"
              type={show ? "text" : "password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              autoFocus
              autoComplete="new-password"
            />
            <button
              type="button"
              className="text-xs text-muted-foreground hover:text-foreground"
              onClick={() => setShow((s) => !s)}
            >
              {show ? "Hide" : "Show"} password
            </button>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!password || resetMut.isPending}>
              Reset password
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
