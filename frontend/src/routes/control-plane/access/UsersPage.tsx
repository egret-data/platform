import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  Compass,
  Eye,
  EyeOff,
  KeyRound,
  ListChecks,
  MoreHorizontal,
  Pencil,
  Plus,
  ShieldCheck,
  Trash2,
  UserCheck,
  UserCog,
  UserX,
  Users,
} from "lucide-react";

import {
  type AccessRole,
  type AccessUser,
  assignUserRole,
  createUser,
  deleteUser,
  listRoles,
  listUsers,
  resetUserPassword,
  unassignUserRole,
  updateUser,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { ImpersonateDialog } from "@/components/ImpersonateDialog";
import { lastActiveAdminReason } from "@/components/access/lastActiveAdmin";
import { DeleteUserDialog, DisableUserDialog } from "@/components/access/UserLifecycleDialogs";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
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
import { Switch } from "@/components/ui/switch";
import { TagCombobox } from "@/components/ui/tag-combobox";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { runBulk } from "@/lib/runBulk";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { ENTITY_LINK_IN_GROUP_CLASS } from "@/components/ui/entity-link";
import { StatusBadge } from "@/components/ui/status-badge";

export const usersQueryKey = ["control-plane", "access", "users"] as const;
const rolesQueryKey = ["control-plane", "access", "roles"] as const;

const PROVIDER_OPTIONS = [
  { value: "local", label: "Local" },
  { value: "oidc", label: "OIDC" },
];

// Loose client-side email shape check — just enough to gate the submit button;
// the backend remains the source of truth.
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/**
 * A required-field label: the `<Label>` keeps clean text (so getByLabelText and
 * the accessible name stay exact) with a decorative red asterisk beside it.
 */
function RequiredLabel({ htmlFor, children }: { htmlFor: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-0.5">
      <Label htmlFor={htmlFor}>{children}</Label>
      <span className="text-destructive" aria-hidden>
        *
      </span>
    </div>
  );
}

/** Two-letter avatar initials from the name (or email when unnamed). */
function initials(u: AccessUser): string {
  const name = u.name?.trim();
  if (name) {
    const parts = name.split(/\s+/).filter(Boolean);
    if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
    return name.slice(0, 2).toUpperCase();
  }
  return (u.email ?? "?").slice(0, 2).toUpperCase();
}

/**
 * Control Plane → Access control → Users.
 *
 * Lists platform accounts (local + SSO), and offers create / edit
 * (name + enable-disable) / role-assignment actions.
 */
export default function UsersPage() {
  const navigate = useNavigate();
  const { me } = useAuth();
  // Role assignment is platform_admin-only on the backend; user create/edit
  // (name + enable/disable) stays delegatable via user:create/user:update.
  const isAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const users = useQuery<AccessUser[], ApiError>({
    queryKey: usersQueryKey,
    queryFn: listUsers,
    retry: false,
  });
  const rows = users.data ?? [];

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<AccessUser | null>(null);
  const [rolesTarget, setRolesTarget] = useState<AccessUser | null>(null);
  const [resetTarget, setResetTarget] = useState<AccessUser | null>(null);
  const [impersonateTarget, setImpersonateTarget] = useState<AccessUser | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AccessUser | null>(null);
  const [disableTarget, setDisableTarget] = useState<AccessUser | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [statusFacet, setStatusFacet] = useState(FACET_ALL);
  const [providerFacet, setProviderFacet] = useState(FACET_ALL);

  const qc = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (id) => deleteUser(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: usersQueryKey });
      setDeleteError(null);
    },
    onError: (err) => setDeleteError(err.message ?? "Failed to delete user."),
  });
  const statusMutation = useMutation<AccessUser, ApiError, { id: string; status: "active" | "disabled" }>({
    mutationFn: ({ id, status }) => updateUser(id, { status }),
    onSuccess: (_, { status }) => {
      qc.invalidateQueries({ queryKey: usersQueryKey });
      toast.success(status === "active" ? "User enabled" : "User disabled");
    },
    onError: (err) => toast.error(err.message ?? "Failed to update status."),
  });

  const columns: Column<AccessUser>[] = [
    {
      // Identity cell: avatar + name (primary) with the email as a secondary
      // line. `value` covers both so search and sort work on either. When a user
      // has no name, the email becomes the primary line.
      key: "user",
      label: "User",
      sortable: true,
      value: (u) => `${u.name ?? ""} ${u.email}`.trim(),
      cell: (u) => (
        <Link to={`/control-plane/users/${u.id}`} className="group flex items-center gap-3 text-left">
          <Avatar className="h-8 w-8">
            <AvatarFallback className="bg-primary/10 text-xs font-medium text-primary">
              {initials(u)}
            </AvatarFallback>
          </Avatar>
          <div className="min-w-0">
            <div className={`truncate ${ENTITY_LINK_IN_GROUP_CLASS}`}>
              {u.name?.trim() || u.email}
            </div>
            {u.name?.trim() ? (
              <div className="truncate text-xs text-muted-foreground">{u.email}</div>
            ) : null}
          </div>
        </Link>
      ),
    },
    { key: "provider", label: "Provider", cell: (u) => <Badge variant="outline">{u.provider}</Badge> },
    {
      key: "status",
      label: "Status",
      cell: (u) =>
        u.status === "active" ? (
          <StatusBadge tone="success">{u.status}</StatusBadge>
        ) : (
          <StatusBadge tone="inactive">{u.status}</StatusBadge>
        ),
    },
    {
      key: "roles",
      label: "Roles",
      // Cap to two chips + a "+N" overflow so a heavily-assigned user never
      // blows out the row width; the overflow badge lists the rest on hover.
      cell: (u) =>
        u.roles.length === 0 ? (
          <span className="text-muted-foreground">—</span>
        ) : (
          <div className="flex flex-wrap items-center gap-1">
            {u.roles.slice(0, 2).map((r) => (
              <Badge key={r} variant="secondary" className="max-w-[10rem] truncate" title={r}>
                {r}
              </Badge>
            ))}
            {u.roles.length > 2 ? (
              <Badge variant="outline" title={u.roles.slice(2).join(", ")}>
                +{u.roles.length - 2}
              </Badge>
            ) : null}
          </div>
        ),
    },
    {
      key: "groups",
      label: "Groups",
      // Groups are mostly empty and secondary here — show a count chip (full
      // list on hover) instead of a wide column of chips.
      cell: (u) =>
        u.groups.length === 0 ? (
          <span className="text-muted-foreground">—</span>
        ) : (
          <Badge
            variant="outline"
            className="cursor-default tabular-nums"
            title={u.groups.join(", ")}
          >
            {u.groups.length}
          </Badge>
        ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (u) => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" aria-label={`Actions for ${u.email}`}>
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              onClick={() => navigate(`/control-plane/access-explorer?user=${u.id}`)}
            >
              <Compass className="mr-2 h-4 w-4" />
              View access
            </DropdownMenuItem>
            {isAdmin && (
              <DropdownMenuItem onClick={() => setRolesTarget(u)}>
                <ShieldCheck className="mr-2 h-4 w-4" />
                Assign roles
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={() => setEditTarget(u)}>
              <Pencil className="mr-2 h-4 w-4" />
              Edit
            </DropdownMenuItem>
            {u.provider === "local" && (
              <DropdownMenuItem onClick={() => setResetTarget(u)}>
                <KeyRound className="mr-2 h-4 w-4" />
                Reset password
              </DropdownMenuItem>
            )}
            {/* Impersonate is a platform_admin-only capability. Hidden (not just
                disabled) for the caller's own row and for platform_admin targets —
                the backend rejects both, but a visible-then-403 action reads as a
                bug rather than a rule. */}
            {isAdmin && u.id !== me?.user_id && !u.roles.includes("platform_admin") && (
              <DropdownMenuItem onClick={() => setImpersonateTarget(u)}>
                <UserCog className="mr-2 h-4 w-4" />
                Impersonate
              </DropdownMenuItem>
            )}
            <DropdownMenuSeparator />
            {/* The detail page's guards: admins only, never yourself. Enable
                needs no confirmation; Disable signs the user out (KI-209).
                On your own row Disable stays listed, aria-disabled, with the
                reason in the item (FE-ACTION-4). */}
            {isAdmin &&
              (u.status === "active" ? (
                <DropdownMenuItem
                  disabled={u.id === me?.user_id}
                  className="items-start"
                  onClick={() => setDisableTarget(u)}
                >
                  <UserX className="mr-2 h-4 w-4" />
                  <span className="flex flex-col">
                    <span>Disable</span>
                    {u.id === me?.user_id ? (
                      <span className="text-xs text-muted-foreground">You cannot disable your own account</span>
                    ) : null}
                  </span>
                </DropdownMenuItem>
              ) : (
                <DropdownMenuItem
                  onClick={() => statusMutation.mutate({ id: u.id, status: "active" })}
                >
                  <UserCheck className="mr-2 h-4 w-4" />
                  Enable
                </DropdownMenuItem>
              ))}
            <DropdownMenuItem
              disabled={u.id === me?.user_id}
              title={u.id === me?.user_id ? "You cannot delete your own account" : undefined}
              className="text-destructive focus:text-destructive"
              onClick={() => {
                setDeleteError(null);
                setDeleteTarget(u);
              }}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ];

  const filtered = rows.filter(
    (u) =>
      (statusFacet === FACET_ALL || u.status === statusFacet) &&
      (providerFacet === FACET_ALL || u.provider === providerFacet),
  );
  const total = rows.length;
  const active = rows.filter((u) => u.status === "active").length;

  // What the TABLE drew, which is not `filtered`: the search box and the
  // `pageSize={25}` pager live inside `FilterableTable`, so `filtered` is a
  // superset of what is on screen. Feeding that superset to `useBulkSelect`
  // broke the contract its own doc comment states — a row picked while visible
  // and then searched away stayed selected, and Disable still reached it.
  const [drawnIds, setDrawnIds] = useState<string[]>([]);
  const drawnRows = useMemo(() => {
    const drawn = new Set(drawnIds);
    return filtered.filter((u) => drawn.has(u.id));
  }, [filtered, drawnIds]);

  const bulk = useBulkSelect(drawnRows);
  const refreshUsers = () => qc.invalidateQueries({ queryKey: usersQueryKey });
  // From `selectedIds`, which re-derives from the rows above — not from
  // `selected` over the full list, which is what reached invisible rows.
  const selectedUsers = () => {
    const chosen = new Set(bulk.selectedIds);
    return drawnRows.filter((u) => chosen.has(u.id));
  };
  const [bulkRoleOpen, setBulkRoleOpen] = useState(false);

  async function bulkSetStatus(status: "active" | "disabled") {
    const targets = selectedUsers();
    await runBulk({
      items: targets,
      op: (u) => updateUser(u.id, { status }),
      undo: (u) => updateUser(u.id, { status: u.status }),
      verb: status === "disabled" ? "Disabled" : "Enabled",
      noun: "users",
      refresh: refreshUsers,
    });
    bulk.clear();
  }

  return (
    <PageContainer className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <PageTitle>Users</PageTitle>
          <p className="text-sm text-muted-foreground">
            Platform accounts, their status, and role assignments.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant={bulk.bulkMode ? "secondary" : "outline"}
            onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
            aria-pressed={bulk.bulkMode}
          >
            <ListChecks className="mr-1 h-4 w-4" /> {bulk.bulkMode ? "Done" : "Bulk select"}
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus className="mr-1 h-4 w-4" />
            New user
          </Button>
        </div>
      </div>

      <div className="space-y-4">
          {deleteError && (
            <Alert variant="destructive">
              <AlertDescription>{deleteError}</AlertDescription>
            </Alert>
          )}
          <SummaryStrip
            items={[
              { label: "users", value: total },
              {
                label: "active",
                value: active,
                active: statusFacet === "active",
                onClick: () =>
                  setStatusFacet((f) => (f === "active" ? FACET_ALL : "active")),
              },
              {
                label: "disabled",
                value: total - active,
                active: statusFacet === "disabled",
                onClick: () =>
                  setStatusFacet((f) => (f === "disabled" ? FACET_ALL : "disabled")),
              },
            ]}
          />
          {bulk.bulkMode && bulk.someSelected && (
            <div className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/40 p-2 text-sm">
              <span className="font-medium">{bulk.selectedIds.length} selected</span>
              <Button size="sm" variant="outline" onClick={() => bulkSetStatus("disabled")}>
                Disable
              </Button>
              <Button size="sm" variant="outline" onClick={() => bulkSetStatus("active")}>
                Enable
              </Button>
              {isAdmin && (
                <Button size="sm" variant="outline" onClick={() => setBulkRoleOpen(true)}>
                  Assign role…
                </Button>
              )}
              <Button size="sm" variant="ghost" className="ml-auto" onClick={bulk.clear}>
                Clear
              </Button>
            </div>
          )}
          {users.isLoading ? (
            <ListSkeleton rows={6} cols={6} />
          ) : (
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(u) => u.id}
              searchPlaceholder="Search users…"
              initialSort={{ key: "user", dir: "asc" }}
              onDrawnChange={setDrawnIds}
              pageSize={25}
              bordered
              emptyText={
                users.isError ? (
                  (users.error?.message ?? "Failed to load users.")
                ) : (
                  <EmptyState
                    icon={Users}
                    title="No users yet"
                    description="Add a local account, or invite people to sign in with SSO."
                    action={
                      <Button size="sm" onClick={() => setCreateOpen(true)}>
                        <Plus className="mr-1 h-4 w-4" />
                        New user
                      </Button>
                    }
                  />
                )
              }
              selection={
                bulk.bulkMode ? { selected: bulk.selected, onToggle: bulk.toggle } : undefined
              }
              filters={
                <>
                  <FacetSelect
                    label="Status"
                    value={statusFacet}
                    onChange={setStatusFacet}
                    options={[
                      { value: "active", label: "Active" },
                      { value: "disabled", label: "Disabled" },
                    ]}
                  />
                  <FacetSelect
                    label="Provider"
                    value={providerFacet}
                    onChange={setProviderFacet}
                    options={PROVIDER_OPTIONS}
                  />
                </>
              }
            />
          )}
      </div>

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
      <ResetPasswordDialog
        user={resetTarget}
        onOpenChange={(o) => {
          if (!o) setResetTarget(null);
        }}
      />
      {impersonateTarget && (
        <ImpersonateDialog
          open
          onOpenChange={(o) => {
            if (!o) setImpersonateTarget(null);
          }}
          targetUserId={impersonateTarget.id}
          targetEmail={impersonateTarget.email}
        />
      )}
      <BulkAssignRoleDialog
        open={bulkRoleOpen}
        users={selectedUsers()}
        onOpenChange={setBulkRoleOpen}
        refresh={refreshUsers}
        onDone={() => bulk.clear()}
      />
      <DeleteUserDialog
        user={deleteTarget}
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
        onDisableInstead={() => {
          setDisableTarget(deleteTarget);
          setDeleteTarget(null);
        }}
        canDisable={isAdmin}
        disableBlockedReason={lastActiveAdminReason(deleteTarget, users.data ?? [])}
      />
      <DisableUserDialog
        user={disableTarget}
        open={disableTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDisableTarget(null);
        }}
        onConfirm={() => {
          if (disableTarget) statusMutation.mutate({ id: disableTarget.id, status: "disabled" });
          setDisableTarget(null);
        }}
      />
    </PageContainer>
  );
}

function BulkAssignRoleDialog({
  open,
  users,
  onOpenChange,
  refresh,
  onDone,
}: {
  open: boolean;
  users: AccessUser[];
  onOpenChange: (open: boolean) => void;
  refresh: () => void;
  onDone: () => void;
}) {
  const roles = useQuery<AccessRole[], ApiError>({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
  });
  const [roleId, setRoleId] = useState("");
  const [saving, setSaving] = useState(false);

  async function handleAssign() {
    if (!roleId) return;
    setSaving(true);
    try {
      await runBulk({
        items: users,
        op: (u) => assignUserRole(u.id, roleId),
        undo: (u) => unassignUserRole(u.id, roleId),
        verb: "Assigned a role to",
        noun: "users",
        refresh,
      });
      onDone();
      onOpenChange(false);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assign role</DialogTitle>
          <DialogDescription>
            Add a role to the {users.length} selected user{users.length === 1 ? "" : "s"}.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-1.5">
          <Label htmlFor="bulk-assign-role">Role</Label>
          <Select value={roleId} onValueChange={setRoleId}>
            <SelectTrigger id="bulk-assign-role" className="w-full">
              <SelectValue placeholder="Select a role…" />
            </SelectTrigger>
            <SelectContent>
              {(roles.data ?? []).map((r) => (
                <SelectItem key={r.id} value={r.id}>
                  {r.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={handleAssign} disabled={saving || !roleId}>
            Assign
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
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
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const createMutation = useMutation<
    AccessUser,
    ApiError,
    { email: string; name: string; password: string }
  >({
    mutationFn: (input) => createUser(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: usersQueryKey }),
  });

  // Gate submit on a filled, plausibly-valid form — matching the other create
  // dialogs — so "Create" isn't a dead click on an empty form.
  const canCreate =
    EMAIL_RE.test(email.trim()) &&
    name.trim().length > 0 &&
    password.length > 0 &&
    !createMutation.isPending;

  function reset() {
    setEmail("");
    setName("");
    setPassword("");
    setShowPassword(false);
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
              <RequiredLabel htmlFor="new-user-email">Email</RequiredLabel>
              <Input
                id="new-user-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
                autoComplete="off"
              />
            </div>

            <div className="space-y-1.5">
              <RequiredLabel htmlFor="new-user-name">Name</RequiredLabel>
              <Input
                id="new-user-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoComplete="name"
              />
            </div>

            <div className="space-y-1.5">
              <RequiredLabel htmlFor="new-user-password">Password</RequiredLabel>
              <div className="relative">
                <Input
                  id="new-user-password"
                  type={showPassword ? "text" : "password"}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  required
                  autoComplete="new-password"
                  className="pr-10"
                />
                <button
                  type="button"
                  onClick={() => setShowPassword((s) => !s)}
                  aria-label={showPassword ? "Hide password" : "Show password"}
                  aria-pressed={showPassword}
                  className="absolute inset-y-0 right-0 flex w-10 items-center justify-center text-muted-foreground hover:text-foreground"
                >
                  {showPassword ? (
                    <EyeOff className="h-4 w-4" />
                  ) : (
                    <Eye className="h-4 w-4" />
                  )}
                </button>
              </div>
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canCreate}>
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

function ResetPasswordDialog({
  user,
  onOpenChange,
}: {
  user: AccessUser | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (user) {
      setPassword("");
      setError(null);
    }
  }, [user]);

  const resetMutation = useMutation<void, ApiError, string>({
    mutationFn: (pw) => resetUserPassword(user!.id, pw),
  });

  function handleOpenChange(next: boolean) {
    if (!next) {
      setPassword("");
      setError(null);
      resetMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await resetMutation.mutateAsync(password);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to reset password.");
    }
  }

  return (
    <Dialog open={user !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Reset password — {user?.email}</DialogTitle>
            <DialogDescription>
              Set a new password for this local account. Their existing sessions
              are signed out immediately.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="reset-user-password">New password</Label>
              <Input
                id="reset-user-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                autoFocus
                autoComplete="new-password"
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={resetMutation.isPending}>
              Reset password
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
