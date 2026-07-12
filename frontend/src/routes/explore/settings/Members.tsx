import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Loader2, MoreHorizontal, Trash2, X } from "lucide-react";
import { toast } from "sonner";

import {
  useAddMember,
  useRemoveMember,
  useWorkspaceMembers,
  useWorkspaceRoles,
  type WorkspaceMember,
} from "@/api/members";
import { listPrincipals } from "@/api/policies";
import { useAuth } from "@/auth/useAuth";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/** Narrow the react-query error into an HTTP status if we have one. */
function errorStatus(err: unknown): number | undefined {
  if (err && typeof err === "object" && "status" in err) {
    const s = (err as { status?: unknown }).status;
    if (typeof s === "number") return s;
  }
  return undefined;
}

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

export default function Members() {
  const ws = useParams().workspace!;
  const { me } = useAuth();
  const membersQ = useWorkspaceMembers(ws);
  const rolesQ = useWorkspaceRoles();
  const addMut = useAddMember(ws);
  const removeMut = useRemoveMember(ws);

  const [removeTarget, setRemoveTarget] = useState<WorkspaceMember | null>(null);

  const members = membersQ.data ?? [];
  const roles = rolesQ.data ?? [];
  const forbidden = errorStatus(membersQ.error) === 403;

  const adminCount = useMemo(
    () => members.filter((m) => m.role === ADMIN_ROLE).length,
    [members],
  );

  const summary = useMemo(() => {
    const people = members.filter((m) => m.principal_type === "user").length;
    const groups = members.filter((m) => m.principal_type === "group").length;
    return `${members.length} member${members.length === 1 ? "" : "s"} · ${adminCount} admin${adminCount === 1 ? "" : "s"} · ${people} ${people === 1 ? "person" : "people"} · ${groups} group${groups === 1 ? "" : "s"}`;
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
        </div>
        <div
          className="rounded-md border border-highlight/40 bg-highlight/5 px-3 py-2 text-xs text-muted-foreground"
          role="note"
        >
          Workspace roles apply only inside <span className="font-medium">{ws}</span>.
          Platform-wide access is granted separately in Control Plane → Access control.
        </div>
      </header>

      {membersQ.isLoading ? (
        <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading members…
        </div>
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
          <AddMemberRow
            roles={roles}
            pending={addMut.isPending}
            onAdd={(body) =>
              addMut.mutate(body, {
                onSuccess: () => toast.success("Member added"),
                onError: (e) => toast.error(String(e)),
              })
            }
          />

          <p className="text-sm text-muted-foreground">{summary}</p>

          <FilterableTable
            rows={members}
            columns={columns}
            getId={(m) => `${m.principal_type}:${m.principal_id}`}
            searchPlaceholder="Search members…"
            initialSort={{ key: "member", dir: "asc" }}
            emptyText="No members yet."
          />
        </>
      )}

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
    </div>
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
    return (
      <div className="flex flex-col gap-0.5">
        <span className="inline-flex h-8 w-40 items-center gap-2 rounded-md border bg-muted/40 px-3 text-sm text-muted-foreground">
          <RoleDot role={member.role} />
          {member.role}
        </span>
        <span className="text-xs text-muted-foreground">Last admin — role locked</span>
      </div>
    );
  }
  return (
    <Select value={member.role} onValueChange={onChange}>
      <SelectTrigger className="h-8 w-40" aria-label={`Role for ${member.principal_name ?? member.principal_id}`}>
        <span className="flex items-center gap-2">
          <RoleDot role={member.role} />
          <SelectValue />
        </span>
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
 * Add-a-member row: a principal-type toggle, a search/select for the principal
 * (suggestions from the policy principal picker), and a workspace-role select.
 * Adapted from ShareDialog's AddGrantRow — the picked principal's `name` is sent
 * as `principal_id`, matching what the members POST endpoint expects.
 */
function AddMemberRow({
  roles,
  pending,
  onAdd,
}: {
  roles: { name: string }[];
  pending: boolean;
  onAdd: (body: { principal_type: string; principal_id: string; role: string }) => void;
}) {
  const [type, setType] = useState<"user" | "group">("user");
  const [value, setValue] = useState("");
  const [role, setRole] = useState<string>("");
  const [showSuggest, setShowSuggest] = useState(false);

  const effectiveRole = role || roles.find((r) => r.name.includes("viewer"))?.name || roles[0]?.name || "";

  const suggestQ = useQuery({
    queryKey: ["member-principals", type, value],
    queryFn: () => listPrincipals(value, type),
    enabled: showSuggest && value.trim().length > 0,
  });
  const suggestions = useMemo(() => suggestQ.data ?? [], [suggestQ.data]);
  const canAdd = value.trim().length > 0 && !!effectiveRole && !pending;

  const submit = () => {
    if (!canAdd) return;
    onAdd({ principal_type: type, principal_id: value.trim(), role: effectiveRole });
    setValue("");
    setShowSuggest(false);
  };

  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/30 p-3">
      <Select
        value={type}
        onValueChange={(v) => {
          setType(v as "user" | "group");
          setValue("");
        }}
      >
        <SelectTrigger className="h-9 w-24" aria-label="Principal type">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="user">User</SelectItem>
          <SelectItem value="group">Group</SelectItem>
        </SelectContent>
      </Select>

      <div className="relative min-w-[12rem] flex-1">
        <Input
          className="h-9"
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
              submit();
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

      <Select value={effectiveRole} onValueChange={setRole}>
        <SelectTrigger className="h-9 w-40" aria-label="Role">
          <span className="flex items-center gap-2">
            {effectiveRole && <RoleDot role={effectiveRole} />}
            <SelectValue placeholder="Role…" />
          </span>
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

      <Button onClick={submit} disabled={!canAdd}>
        {pending ? <Loader2 className="h-4 w-4 animate-spin" /> : "Add member"}
      </Button>

      {value.trim().length > 0 && showSuggest && (
        <button
          type="button"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setShowSuggest(false)}
        >
          <X className="h-3 w-3" /> hide
        </button>
      )}
    </div>
  );
}
