import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Pencil, Plus, ShieldCheck } from "lucide-react";

import {
  type AccessRole,
  type AccessUser,
  assignUserRole,
  createUser,
  listRoles,
  listUsers,
  unassignUserRole,
  updateUser,
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
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { TagCombobox } from "@/components/ui/tag-combobox";

export const usersQueryKey = ["control-plane", "access", "users"] as const;
const rolesQueryKey = ["control-plane", "access", "roles"] as const;

const COLUMNS = ["Email", "Name", "Provider", "Status", "Roles", "Groups", ""];

/**
 * Control Plane → Access control → Users.
 *
 * Lists platform accounts (local + SSO), and offers create / edit
 * (name + enable-disable) / role-assignment actions.
 */
export default function UsersPage() {
  const users = useQuery<AccessUser[], ApiError>({
    queryKey: usersQueryKey,
    queryFn: listUsers,
    retry: false,
  });
  const rows = users.data ?? [];

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<AccessUser | null>(null);
  const [rolesTarget, setRolesTarget] = useState<AccessUser | null>(null);

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Users</h1>
        <p className="text-sm text-muted-foreground">
          Platform accounts, their status, and role assignments.
        </p>
      </div>

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between gap-2">
            <div className="space-y-1.5">
              <CardTitle className="text-lg">All users</CardTitle>
              <CardDescription>
                Local and SSO accounts with access to egret.
              </CardDescription>
            </div>
            <Button size="sm" onClick={() => setCreateOpen(true)}>
              <Plus className="mr-1 h-4 w-4" />
              New user
            </Button>
          </div>
        </CardHeader>
        <CardContent>
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
              {rows.map((u) => (
                <TableRow key={u.id}>
                  <TableCell className="font-medium">{u.email}</TableCell>
                  <TableCell>{u.name}</TableCell>
                  <TableCell>
                    <Badge variant="outline">{u.provider}</Badge>
                  </TableCell>
                  <TableCell>
                    <Badge variant={u.status === "active" ? "default" : "secondary"}>
                      {u.status}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    {u.roles.length === 0 ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <div className="flex flex-wrap gap-1">
                        {u.roles.map((r) => (
                          <Badge key={r} variant="secondary">
                            {r}
                          </Badge>
                        ))}
                      </div>
                    )}
                  </TableCell>
                  <TableCell>
                    {u.groups.length === 0 ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <div className="flex flex-wrap gap-1">
                        {u.groups.map((g) => (
                          <Badge key={g} variant="outline">
                            {g}
                          </Badge>
                        ))}
                      </div>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={`Actions for ${u.email}`}
                        >
                          <MoreHorizontal className="h-4 w-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onClick={() => setRolesTarget(u)}>
                          <ShieldCheck className="mr-2 h-4 w-4" />
                          Assign roles
                        </DropdownMenuItem>
                        <DropdownMenuItem onClick={() => setEditTarget(u)}>
                          <Pencil className="mr-2 h-4 w-4" />
                          Edit
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </TableCell>
                </TableRow>
              ))}
              {rows.length === 0 && (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={COLUMNS.length}
                    className="text-center text-muted-foreground"
                  >
                    {users.isError ? (
                      <span className="text-destructive">
                        {users.error?.message ?? "Failed to load users."}
                      </span>
                    ) : users.isLoading ? (
                      "Loading…"
                    ) : (
                      "No users yet."
                    )}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <CreateUserDialog open={createOpen} onOpenChange={setCreateOpen} />
      <EditUserDialog
        user={editTarget}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
      />
      <AssignRolesDialog
        user={rolesTarget}
        onOpenChange={(o) => {
          if (!o) setRolesTarget(null);
        }}
      />
    </div>
  );
}

function CreateUserDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createMutation = useMutation<
    AccessUser,
    ApiError,
    { email: string; name: string; password: string }
  >({
    mutationFn: (input) => createUser(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersQueryKey }),
  });

  function reset() {
    setEmail("");
    setName("");
    setPassword("");
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
        email: email.trim(),
        name: name.trim(),
        password,
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create user.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New user</DialogTitle>
            <DialogDescription>
              Create a local account with an email and password.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="new-user-email">Email</Label>
              <Input
                id="new-user-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-user-name">Name</Label>
              <Input
                id="new-user-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-user-password">Password</Label>
              <Input
                id="new-user-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                autoComplete="new-password"
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

function EditUserDialog({
  user,
  onOpenChange,
}: {
  user: AccessUser | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (user) {
      setName(user.name);
      setEnabled(user.status === "active");
      setError(null);
    }
  }, [user]);

  const updateMutation = useMutation<AccessUser, ApiError, { name?: string; status?: string }>({
    mutationFn: (input) => updateUser(user!.id, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersQueryKey }),
  });

  function handleOpenChange(next: boolean) {
    if (!next) {
      setError(null);
      updateMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await updateMutation.mutateAsync({
        name: name.trim(),
        status: enabled ? "active" : "disabled",
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to update user.");
    }
  }

  return (
    <Dialog open={user !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Edit {user?.email}</DialogTitle>
            <DialogDescription>
              Update the display name or enable/disable this account.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="edit-user-name">Name</Label>
              <Input
                id="edit-user-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>

            <div className="flex items-center justify-between rounded-md border p-3">
              <div className="space-y-0.5">
                <Label htmlFor="edit-user-enabled">Enabled</Label>
                <p className="text-xs text-muted-foreground">
                  Disabled users cannot sign in.
                </p>
              </div>
              <Switch id="edit-user-enabled" checked={enabled} onCheckedChange={setEnabled} />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={updateMutation.isPending}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function AssignRolesDialog({
  user,
  onOpenChange,
}: {
  user: AccessUser | null;
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
    if (user) {
      setSelected(user.roles);
      setError(null);
    }
  }, [user]);

  function handleOpenChange(next: boolean) {
    if (!next) setError(null);
    onOpenChange(next);
  }

  async function handleSave() {
    if (!user) return;
    setError(null);
    setSaving(true);
    const roleByName = new Map((roles.data ?? []).map((r) => [r.name, r]));
    const current = new Set(user.roles);
    const next = new Set(selected);
    const toAdd = selected.filter((r) => !current.has(r));
    const toRemove = user.roles.filter((r) => !next.has(r));
    let failed = false;
    try {
      for (const name of toAdd) {
        const role = roleByName.get(name);
        if (role) await assignUserRole(user.id, role.id);
      }
      for (const name of toRemove) {
        const role = roleByName.get(name);
        if (role) await unassignUserRole(user.id, role.id);
      }
    } catch (err) {
      failed = true;
      setError((err as ApiError).message ?? "Failed to update roles.");
    } finally {
      queryClient.invalidateQueries({ queryKey: usersQueryKey });
      setSaving(false);
    }
    if (!failed) handleOpenChange(false);
  }

  return (
    <Dialog open={user !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assign roles — {user?.email}</DialogTitle>
          <DialogDescription>Choose the roles this user should have.</DialogDescription>
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
