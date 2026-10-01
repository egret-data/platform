import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ListChecks, Loader2, Lock, MoreHorizontal, Trash2, UserCog, UserPlus } from "lucide-react";
import { toast } from "sonner";

import {
  membersKey,
  useAddMember,
  useRemoveMember,
  useWorkspaceMembers,
  useWorkspaceRoles,
  type WorkspaceMember,
} from "@/api/members";
import { listPrincipals } from "@/api/policies";
import { useAuth } from "@/auth/useAuth";
import { ImpersonateDialog } from "@/components/ImpersonateDialog";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import { useBulkSelect } from "@/hooks/useBulkSelect";
import { runBulk } from "@/lib/runBulk";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { errorStatus } from "@/lib/httpStatus";

/** Small dot colour for a role, using semantic design tokens. */
function roleDotColor(role: string): string {
  if (role.includes("admin")) return "var(--success)";
  if (role.includes("editor")) return "var(--primary)";
  return "var(--muted-foreground)";
}

/** A coloured swatch preceding a role name. */
function RoleDot({ role }: { role: string }) {
  return (
    <span
      className="inline-block h-2 w-2 shrink-0 rounded-full"
      style={{ background: roleDotColor(role) }}
      aria-hidden
    />
  );
}

const ADMIN_ROLE = "ws_admin";

/**
 * Reusable members-management panel. Renders the full add / change-role / remove
 * UI for a single workspace, keyed by `workspaceId` (the workspace path key used
 * by `/api/ws/{workspace}/members`). Consumed by the workspace-side Members page
 * and the control-plane Workspace detail's Members tab — `platform_admin`
 * bypasses the `workspace:manage_members` gate, so the same panel works in both.
 */
export function MembersPanel({ workspaceId }: { workspaceId: string }) {
  const ws = workspaceId;
  const { me } = useAuth();
  const membersQ = useWorkspaceMembers(ws);
  const rolesQ = useWorkspaceRoles();
  const addMut = useAddMember(ws);
  const removeMut = useRemoveMember(ws);

  const [removeTarget, setRemoveTarget] = useState<WorkspaceMember | null>(null);
  const [impersonateTarget, setImpersonateTarget] = useState<WorkspaceMember | null>(null);
  const [bulkRoleOpen, setBulkRoleOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);

  const members = membersQ.data ?? [];
  const roles = rolesQ.data ?? [];
  const forbidden = errorStatus(membersQ.error) === 403;

  const qc = useQueryClient();
  const mid = (m: WorkspaceMember) => `${m.principal_type}:${m.principal_id}`;
  const bulkRows = useMemo(() => members.map((m) => ({ ...m, id: mid(m) })), [members]);
  // What the TABLE drew, which is not `bulkRows`: `FilterableTable` owns the
  // search box, so `bulkRows` is a superset of what is on screen. Feeding that
  // superset to `useBulkSelect` broke the contract its own doc comment states —
  // a member picked while visible and then searched away stayed selected, and
  // Remove-from-workspace still reached them.
  const [drawnIds, setDrawnIds] = useState<string[]>([]);
  const drawnRows = useMemo(() => {
    const drawn = new Set(drawnIds);
    return bulkRows.filter((m) => drawn.has(m.id));
  }, [bulkRows, drawnIds]);

  const bulk = useBulkSelect(drawnRows);
  // From `selectedIds`, which re-derives from the rows above — not from
  // `selected` over the full list, which is what reached invisible rows.
  const selectedMembers = () => {
    const chosen = new Set(bulk.selectedIds);
    return drawnRows.filter((m) => chosen.has(m.id));
  };
  const refreshMembers = () => qc.invalidateQueries({ queryKey: membersKey(ws) });

  const adminCount = useMemo(
    () => members.filter((m) => m.role === ADMIN_ROLE).length,
    [members],
  );

  const summaryItems = useMemo(() => {
    const people = members.filter((m) => m.principal_type === "user").length;
    const groups = members.filter((m) => m.principal_type === "group").length;
    return [
      { label: "members", value: members.length },
      { label: "admins", value: adminCount },
      { label: "people", value: people },
      { label: "groups", value: groups },
    ];
  }, [members, adminCount]);

  const isSelf = (m: WorkspaceMember) =>
    m.principal_type === "user" &&
    !!me &&
    (m.principal_id === me.user_id || m.principal_id === me.email);

  const changeRole = (m: WorkspaceMember, role: string) => {
    addMut.mutate(
      { principal_type: m.principal_type, principal_id: m.principal_id, role },
      {
        onSuccess: () => toast.success("Role updated"),
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  const confirmRemove = () => {
    if (!removeTarget) return;
    const m = removeTarget;
    removeMut.mutate(
      { principal_type: m.principal_type, principal_id: m.principal_id },
      {
        onSuccess: () => toast.success("Removed from workspace"),
        onError: (e) => toast.error(String(e)),
      },
    );
    setRemoveTarget(null);
  };

  async function bulkRemove() {
    const targets = selectedMembers();
    await runBulk({
      items: targets,
      op: (m) =>
        removeMut.mutateAsync({ principal_type: m.principal_type, principal_id: m.principal_id }),
      undo: (m) =>
        addMut.mutateAsync({
          principal_type: m.principal_type,
          principal_id: m.principal_id,
          role: m.role,
        }),
      verb: "Removed",
      noun: "members",
      refresh: refreshMembers,
    });
    bulk.clear();
  }

  const columns: Column<WorkspaceMember>[] = [
    {
      key: "member",
      label: "Member",
      sortable: true,
      value: (m) => m.principal_name ?? m.principal_id,
      cell: (m) => (
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate font-medium" title={m.principal_id}>
            {m.principal_name ?? m.principal_id}
          </span>
          {m.is_platform_admin && (
            <Badge
              variant="outline"
              className="border-highlight/50 text-highlight"
              title="Also a platform admin — has access to every workspace"
            >
              platform admin
            </Badge>
          )}
        </span>
      ),
    },
    {
      key: "type",
      label: "Type",
      sortable: true,
      value: (m) => m.principal_type,
      cell: (m) => (
        <Badge variant="outline">{m.principal_type === "group" ? "Group" : "User"}</Badge>
      ),
    },
    {
      key: "role",
      label: "Role",
      sortable: true,
      value: (m) => m.role,
      cell: (m) => (
        <RoleCell
          member={m}
          roles={roles}
          locked={m.role === ADMIN_ROLE && adminCount <= 1}
          onChange={(role) => changeRole(m, role)}
        />
      ),
    },
    {
      key: "granted_by",
      label: "Added by",
      sortable: true,
      value: (m) => m.granted_by_name ?? "",
      cell: (m) => (
        <span className="text-sm text-muted-foreground">{m.granted_by_name ?? "—"}</span>
      ),
    },
    {
      key: "added_at",
      label: "Added",
      sortable: true,
      value: (m) => m.added_at,
      cell: (m) => (
        <span className="text-sm text-muted-foreground" title={fullTimestamp(m.added_at)}>
          {timeAgo(m.added_at)}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (m) => {
        const lastAdmin = m.role === ADMIN_ROLE && adminCount <= 1;
        // Impersonate is a ws_admin (or platform_admin) capability, already
        // gated at the panel level (this column only renders when `!forbidden`,
        // i.e. the caller has one of those). Hidden — not just disabled — for
        // the caller's own row, for another ws_admin, for a platform_admin, and
        // for groups: the backend rejects all four, but a visible-then-403
        // action reads as a bug rather than a rule.
        //
        // ⚠ There is now a FIFTH case the backend rejects that this cannot hide:
        // a member holding ANY platform-scoped role grant, not just
        // `platform_admin` (SP4d debt Task 13 — a ws-scoped overlay over a
        // platform `data_steward` could write permission grants on any
        // workspace's catalog, because no `/api/control-plane` route confines
        // one). Such a member still shows the action and gets a 403. Hiding it
        // needs a new field on the member DTO — `is_platform_admin` is the only
        // platform fact the row carries, and it answers a narrower question —
        // which is a backend change this task did not take.
        const canImpersonate =
          m.principal_type === "user" &&
          !isSelf(m) &&
          m.role !== ADMIN_ROLE &&
          !m.is_platform_admin;
        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label={`Actions for ${m.principal_name ?? m.principal_id}`}
              >
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {canImpersonate && (
                <DropdownMenuItem onClick={() => setImpersonateTarget(m)}>
                  <UserCog className="mr-2 h-4 w-4" />
                  Impersonate
                </DropdownMenuItem>
              )}
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                disabled={lastAdmin}
                onClick={() => setRemoveTarget(m)}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Remove from workspace
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        );
      },
    },
  ];

  return (
    <div className="space-y-5">
      <header className="space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="text-lg font-semibold">Members</h2>
            <p className="text-sm text-muted-foreground">
              People and groups with access to <span className="font-medium">{ws}</span>.
            </p>
          </div>
          {!forbidden && !membersQ.isLoading && (
            <div className="flex items-center gap-2">
              <Button size="sm" onClick={() => setAddOpen(true)}>
                <UserPlus className="mr-1 h-4 w-4" /> Add member
              </Button>
              <Button
                size="sm"
                variant={bulk.bulkMode ? "secondary" : "outline"}
                onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
                aria-pressed={bulk.bulkMode}
              >
                <ListChecks className="mr-1 h-4 w-4" /> {bulk.bulkMode ? "Done" : "Bulk select"}
              </Button>
            </div>
          )}
        </div>
        <SettingsNote>
          Workspace roles apply only inside <span className="font-medium">{ws}</span> — see the{" "}
          <span className="font-medium">Roles &amp; permissions</span> tab for what each grants.
          Platform-wide access is granted separately in{" "}
          <span className="font-medium">Control Plane → Access control</span>.
        </SettingsNote>
      </header>

      {membersQ.isLoading ? (
        <ListSkeleton rows={5} cols={6} />
      ) : forbidden ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground" role="alert">
          Only workspace admins can manage members.
        </p>
      ) : membersQ.isError ? (
        <p className="py-6 text-sm text-destructive" role="alert">
          Couldn&apos;t load members. {String(membersQ.error)}
        </p>
      ) : (
        <>
          <SummaryStrip items={summaryItems} />

          {bulk.bulkMode && bulk.someSelected && (
            <div className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/40 p-2 text-sm">
              <span className="font-medium">{bulk.selectedIds.length} selected</span>
              <Button size="sm" variant="outline" onClick={bulkRemove}>
                Remove
              </Button>
              <Button size="sm" variant="outline" onClick={() => setBulkRoleOpen(true)}>
                Set role…
              </Button>
              <Button size="sm" variant="ghost" className="ml-auto" onClick={bulk.clear}>
                Clear
              </Button>
            </div>
          )}

          <FilterableTable
            rows={members}
            columns={columns}
            getId={(m) => `${m.principal_type}:${m.principal_id}`}
            searchPlaceholder="Search members…"
            initialSort={{ key: "member", dir: "asc" }}
            onDrawnChange={setDrawnIds}
            emptyText="No members yet."
            selection={
              bulk.bulkMode ? { selected: bulk.selected, onToggle: bulk.toggle } : undefined
            }
          />
        </>
      )}

      {impersonateTarget && (
        <ImpersonateDialog
          open
          onOpenChange={(o) => {
            if (!o) setImpersonateTarget(null);
          }}
          targetUserId={impersonateTarget.principal_id}
          targetEmail={impersonateTarget.principal_name ?? impersonateTarget.principal_id}
          workspaceScope={ws}
        />
      )}

      <AddMemberDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        roles={roles}
        pending={addMut.isPending}
        onAdd={(body) => addMut.mutateAsync(body)}
      />

      <AlertDialog open={!!removeTarget} onOpenChange={(o) => !o && setRemoveTarget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove from workspace?</AlertDialogTitle>
            <AlertDialogDescription>
              {removeTarget?.principal_name ?? removeTarget?.principal_id} will lose access to{" "}
              <span className="font-medium">{ws}</span>.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {removeTarget && isSelf(removeTarget) && (
            <p className="text-sm font-medium text-destructive">
              You&apos;re removing your own access.
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={confirmRemove}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <BulkSetRoleDialog
        open={bulkRoleOpen}
        roles={roles}
        count={bulk.selectedIds.length}
        onOpenChange={setBulkRoleOpen}
        onConfirm={async (newRole) => {
          const targets = selectedMembers();
          await runBulk({
            items: targets,
            op: (m) =>
              addMut.mutateAsync({
                principal_type: m.principal_type,
                principal_id: m.principal_id,
                role: newRole,
              }),
            undo: (m) =>
              addMut.mutateAsync({
                principal_type: m.principal_type,
                principal_id: m.principal_id,
                role: m.role,
              }),
            verb: "Set role for",
            noun: "members",
            refresh: refreshMembers,
          });
          bulk.clear();
        }}
      />
    </div>
  );
}

/** In-file dialog to pick a workspace role and apply it to the selected members. */
function BulkSetRoleDialog({
  open,
  roles,
  count,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  roles: { name: string }[];
  count: number;
  onOpenChange: (open: boolean) => void;
  onConfirm: (role: string) => Promise<void>;
}) {
  const [role, setRole] = useState("");
  const [saving, setSaving] = useState(false);

  async function handleApply() {
    if (!role) return;
    setSaving(true);
    try {
      await onConfirm(role);
      onOpenChange(false);
      setRole("");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Set role</DialogTitle>
          <DialogDescription>
            Apply a workspace role to the {count} selected member{count === 1 ? "" : "s"}.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-1.5">
          <Label htmlFor="bulk-set-role">Role</Label>
          <Select value={role} onValueChange={setRole}>
            <SelectTrigger id="bulk-set-role" className="w-full">
              <SelectValue placeholder="Select a role…" />
            </SelectTrigger>
            <SelectContent>
              {roles.map((r) => (
                <SelectItem key={r.name} value={r.name}>
                  <span className="flex items-center gap-2">
                    <RoleDot role={r.name} />
                    {r.name}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={handleApply} disabled={saving || !role}>
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Inline role editor with a last-admin guardrail. */
function RoleCell({
  member,
  roles,
  locked,
  onChange,
}: {
  member: WorkspaceMember;
  roles: { name: string }[];
  locked: boolean;
  onChange: (role: string) => void;
}) {
  if (locked) {
    // A static pill the same height as the Select trigger so rows stay aligned;
    // the "why" moves into a tooltip instead of a caption that grows the row.
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            tabIndex={0}
            className="inline-flex h-8 w-40 cursor-default items-center gap-2 rounded-md border bg-muted/40 px-3 text-sm text-muted-foreground"
          >
            <RoleDot role={member.role} />
            <span className="truncate">{member.role}</span>
            <Lock className="ml-auto h-3 w-3 shrink-0" aria-hidden />
          </span>
        </TooltipTrigger>
        <TooltipContent>Last admin — role can’t be changed</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <Select value={member.role} onValueChange={onChange}>
      <SelectTrigger className="h-8 w-40" aria-label={`Role for ${member.principal_name ?? member.principal_id}`}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {roles.map((r) => (
          <SelectItem key={r.name} value={r.name}>
            <span className="flex items-center gap-2">
              <RoleDot role={r.name} />
              {r.name}
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * Add-a-member dialog: a principal-type toggle, a search/select for the
 * principal (suggestions from the policy principal picker), and a workspace-role
 * select. Opened from the header's "Add member" button so the form stays out of
 * the way until needed. Adapted from ShareDialog's AddGrantRow — the picked
 * principal's `name` is sent as `principal_id`, matching the members POST
 * endpoint.
 */
function AddMemberDialog({
  open,
  onOpenChange,
  roles,
  pending,
  onAdd,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  roles: { name: string }[];
  pending: boolean;
  onAdd: (body: { principal_type: string; principal_id: string; role: string }) => Promise<unknown>;
}) {
  const [type, setType] = useState<"user" | "group">("user");
  const [value, setValue] = useState("");
  const [role, setRole] = useState<string>("");
  const [showSuggest, setShowSuggest] = useState(false);

  // Reset the form each time the dialog is (re)opened.
  useEffect(() => {
    if (open) {
      setType("user");
      setValue("");
      setRole("");
      setShowSuggest(false);
    }
  }, [open]);

  const effectiveRole =
    role || roles.find((r) => r.name.includes("viewer"))?.name || roles[0]?.name || "";

  const suggestQ = useQuery({
    queryKey: ["member-principals", type, value],
    queryFn: () => listPrincipals(value, type),
    enabled: open && showSuggest && value.trim().length > 0,
  });
  const suggestions = useMemo(() => suggestQ.data ?? [], [suggestQ.data]);
  const canAdd = value.trim().length > 0 && !!effectiveRole && !pending;

  const submit = async () => {
    if (!canAdd) return;
    try {
      await onAdd({ principal_type: type, principal_id: value.trim(), role: effectiveRole });
      toast.success("Member added");
      onOpenChange(false);
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add member</DialogTitle>
          <DialogDescription>Give a user or group a role in this workspace.</DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <div className="space-y-1.5">
            <Label htmlFor="add-member-type">Type</Label>
            <Select
              value={type}
              onValueChange={(v) => {
                setType(v as "user" | "group");
                setValue("");
              }}
            >
              <SelectTrigger id="add-member-type" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="user">User</SelectItem>
                <SelectItem value="group">Group</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="add-member-principal">{type === "group" ? "Group name" : "User email"}</Label>
            <div className="relative">
              <Input
                id="add-member-principal"
                autoFocus
                aria-label={type === "group" ? "Group name" : "User email"}
                placeholder={type === "group" ? "Group name…" : "User email…"}
                value={value}
                onChange={(e) => {
                  setValue(e.target.value);
                  setShowSuggest(true);
                }}
                onFocus={() => setShowSuggest(true)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void submit();
                  }
                }}
              />
              {showSuggest && value.trim().length > 0 && suggestions.length > 0 && (
                <ul className="absolute z-10 mt-1 max-h-48 w-full overflow-auto rounded-md border bg-popover p-1 shadow-md">
                  {suggestions.map((p) => (
                    <li key={p.name}>
                      <button
                        type="button"
                        className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                        onClick={() => {
                          setValue(p.name);
                          setShowSuggest(false);
                        }}
                      >
                        <span className="font-medium">{p.label}</span>
                        {p.label !== p.name && (
                          <span className="ml-1 text-xs text-muted-foreground">{p.name}</span>
                        )}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="add-member-role">Role</Label>
            <Select value={effectiveRole} onValueChange={setRole}>
              <SelectTrigger id="add-member-role" className="w-full" aria-label="Role">
                <SelectValue placeholder="Role…" />
              </SelectTrigger>
              <SelectContent>
                {roles.map((r) => (
                  <SelectItem key={r.name} value={r.name}>
                    <span className="flex items-center gap-2">
                      <RoleDot role={r.name} />
                      {r.name}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={!canAdd}>
            {pending ? <Loader2 className="h-4 w-4 animate-spin" /> : "Add member"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
