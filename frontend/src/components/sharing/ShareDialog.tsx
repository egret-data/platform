import { useId, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { listPrincipals } from "@/api/policies";
import { errorStatus } from "@/lib/httpStatus";
import {
  getSharesGrantListQueryKey,
  useSharesGrantDelete,
  useSharesGrantList,
  useSharesGrantPut,
} from "@/api/generated/shares/shares";
import type { ObjectGrantDto } from "@/api/generated/model";

/** Object kinds that support object-scoped sharing. */
export type ShareKind = "dashboard" | "chart" | "dataset" | "folder" | "saved_query" | "app";

/** Workspace roles offered when sharing. */
const ROLE_OPTIONS = [
  { value: "ws_viewer", label: "Viewer" },
  { value: "ws_editor", label: "Editor" },
] as const;

/** Sentinel role value for an explicit "deny" (Blocked) grant. */
const BLOCKED_VALUE = "__blocked__";

/**
 * Options for the add-grant role select: the two allow roles plus a "Blocked"
 * choice that maps to a `ws_editor` deny grant (deny-wins over any allow).
 */
const ADD_ROLE_OPTIONS = [
  ...ROLE_OPTIONS,
  { value: BLOCKED_VALUE, label: "Blocked (No access)" },
] as const;

const roleLabel = (role: string) =>
  ROLE_OPTIONS.find((r) => r.value === role)?.label ?? role;

/** Body of a share grant PUT. */
interface GrantBody {
  principal_type: string;
  principal_id: string;
  role: string;
  effect: string;
  /** RFC3339 timestamp. Omitted (undefined) = no expiry. */
  expires_at?: string;
}

/**
 * Map a selected role option to a grant body. The "Blocked" sentinel becomes a
 * `ws_editor` deny grant; a real role is an allow grant. `expiresAt` is an
 * end-of-local-day RFC3339 string, or empty/undefined for no expiry.
 */
function grantBody(
  principal_type: string,
  principal_id: string,
  role: string,
  expiresAt?: string,
): GrantBody {
  const base =
    role === BLOCKED_VALUE
      ? { principal_type, principal_id, role: "ws_editor", effect: "deny" }
      : { principal_type, principal_id, role, effect: "allow" };
  return expiresAt ? { ...base, expires_at: expiresAt } : base;
}

/** Convert a `yyyy-mm-dd` date-input value to an end-of-local-day RFC3339 timestamp. */
function toExpiresAt(dateValue: string): string | undefined {
  if (!dateValue) return undefined;
  return new Date(`${dateValue}T23:59:59`).toISOString();
}

/** Render an ISO timestamp's date portion, e.g. "2030-06-01". */
function expiryDateLabel(iso: string): string {
  return iso.slice(0, 10);
}

/** Red "No access" badge shown for a deny grant, using the destructive token. */
function NoAccessBadge() {
  return (
    <Badge
      className="border-transparent text-white"
      style={{ backgroundColor: "var(--destructive)" }}
    >
      No access
    </Badge>
  );
}

interface Props {
  ws: string;
  kind: ShareKind;
  id: string;
  /** When set, inherited grants from this folder are shown read-only. */
  folderId?: string | null;
  /** Human name of the object, shown in the header. */
  name?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Metabase-style sharing modal for an object (dashboard / chart / dataset /
 * folder). Lists direct grants (editable), inherited-from-folder grants
 * (read-only), and lets a manager add or remove direct grants.
 *
 * The grants list endpoint is manage-gated: a non-manager receives a 403,
 * which we surface as a friendly message rather than crashing or leaking the
 * ACL.
 */
export function ShareDialog({ ws, kind, id, folderId, name, open, onOpenChange }: Props) {
  const qc = useQueryClient();

  const directKey = getSharesGrantListQueryKey(ws, kind, id);
  const directQ = useSharesGrantList(ws, kind, id, {
    query: { enabled: open, retry: false },
  });

  const inheritedQ = useSharesGrantList(ws, "folder", folderId ?? "", {
    query: { enabled: open && !!folderId, retry: false },
  });

  const put = useSharesGrantPut();
  const del = useSharesGrantDelete();

  const direct = (directQ.data?.data ?? []) as ObjectGrantDto[];
  const inherited = (inheritedQ.data?.data ?? []) as ObjectGrantDto[];
  const forbidden = errorStatus(directQ.error) === 403;

  const invalidate = () => qc.invalidateQueries({ queryKey: directKey });

  const upsertRole = (grant: ObjectGrantDto, role: string) => {
    put.mutate(
      {
        workspace: ws,
        kind,
        id,
        data: grantBody(grant.principal_type, grant.principal_id, role),
      },
      {
        onSuccess: invalidate,
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  const removeGrant = (grant: ObjectGrantDto) => {
    del.mutate(
      { workspace: ws, kind, id, grantId: grant.id },
      {
        onSuccess: () => {
          toast.success("Access removed");
          invalidate();
        },
        onError: (e) => toast.error(String(e)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Share{name ? ` "${name}"` : ""}</DialogTitle>
          <DialogDescription>
            Grant people and groups access to this {kind}.
          </DialogDescription>
        </DialogHeader>

        {directQ.isLoading ? (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading access…
          </div>
        ) : forbidden ? (
          <p className="py-6 text-sm text-muted-foreground" role="alert">
            You don&apos;t have permission to manage sharing for this {kind}.
          </p>
        ) : directQ.isError ? (
          <p className="py-6 text-sm text-destructive" role="alert">
            Couldn&apos;t load sharing settings. {String(directQ.error)}
          </p>
        ) : (
          <div className="space-y-5">
            <AddGrantRow
              onAdd={(body) =>
                put.mutate(
                  { workspace: ws, kind, id, data: body },
                  {
                    onSuccess: () => {
                      toast.success("Access granted");
                      invalidate();
                    },
                    onError: (e) => toast.error(String(e)),
                  },
                )
              }
              pending={put.isPending}
            />

            <section aria-label="Direct access" className="space-y-2">
              <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                People &amp; groups with access
              </h3>
              {direct.length === 0 ? (
                <p className="text-sm text-muted-foreground">No direct access yet.</p>
              ) : (
                <ul className="divide-y rounded-md border">
                  {direct.map((g) => (
                    <li key={g.id} className="flex items-center gap-2 px-3 py-2">
                      <PrincipalLabel grant={g} />
                      {g.expires_at && (
                        <span
                          className="shrink-0 text-xs text-muted-foreground"
                          title="Expiry can't be edited in place; remove and re-add the grant to change it"
                        >
                          expires {expiryDateLabel(g.expires_at)}
                        </span>
                      )}
                      <div className="ml-auto flex items-center gap-1">
                        {g.effect === "deny" ? (
                          <NoAccessBadge />
                        ) : (
                          <Select value={g.role} onValueChange={(v) => upsertRole(g, v)}>
                            <SelectTrigger
                              className="h-8 w-28"
                              aria-label={`Role for ${principalLabel(g)}`}
                            >
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              {ROLE_OPTIONS.map((r) => (
                                <SelectItem key={r.value} value={r.value}>
                                  {r.label}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        )}
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-8 w-8 text-muted-foreground"
                          aria-label={`Remove ${principalLabel(g)}`}
                          onClick={() => removeGrant(g)}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            {folderId && (
              <section aria-label="Inherited access" className="space-y-2">
                <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  Inherited from folder
                </h3>
                {inheritedQ.isLoading ? (
                  <div className="flex items-center gap-2 text-sm text-muted-foreground">
                    <Loader2 className="h-4 w-4 animate-spin" /> Loading…
                  </div>
                ) : inherited.length === 0 ? (
                  <p className="text-sm text-muted-foreground">
                    No access inherited from the folder.
                  </p>
                ) : (
                  <ul className="divide-y rounded-md border bg-muted/40">
                    {inherited.map((g) => (
                      <li key={g.id} className="flex items-center gap-2 px-3 py-2">
                        <PrincipalLabel grant={g} />
                        {g.effect === "deny" ? (
                          <span className="ml-auto">
                            <NoAccessBadge />
                          </span>
                        ) : (
                          <span className="ml-auto text-xs text-muted-foreground">
                            {roleLabel(g.role)}
                          </span>
                        )}
                      </li>
                    ))}
                  </ul>
                )}
                <p className="text-xs text-muted-foreground">
                  Inherited access is managed on the folder and can&apos;t be changed here.
                </p>
              </section>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Resolved grantee display name (email / group name), falling back to the raw id. */
const principalLabel = (grant: ObjectGrantDto) => grant.principal_name ?? grant.principal_id;

/** Grantee cell: resolved principal name plus a User/Group badge. */
function PrincipalLabel({ grant }: { grant: ObjectGrantDto }) {
  const isGroup = grant.principal_type === "group";
  return (
    <span className="flex min-w-0 items-center gap-2">
      <span className="truncate text-sm" title={grant.principal_id}>
        {principalLabel(grant)}
      </span>
      <Badge variant="outline" className="shrink-0">
        {isGroup ? "Group" : "User"}
      </Badge>
    </span>
  );
}

/**
 * Add-a-grant row: a principal-type toggle, a search/select for the principal
 * (suggestions from the policy principal picker), and a role select.
 */
function AddGrantRow({
  onAdd,
  pending,
}: {
  onAdd: (body: GrantBody) => void;
  pending: boolean;
}) {
  const [type, setType] = useState<"user" | "group">("user");
  const [value, setValue] = useState("");
  const [role, setRole] = useState<string>("ws_viewer");
  const [showSuggest, setShowSuggest] = useState(false);
  // Optional expiry date (yyyy-mm-dd from the date input). Empty = never
  // expires — the unchanged default. This only affects NEW grants: the PUT
  // endpoint is `ON CONFLICT DO NOTHING`, so re-PUTting an existing
  // (principal, object, role) never updates its stored expiry. To change an
  // existing grant's expiry, remove it (Trash2 button below) and re-add it
  // here with the new date.
  const [expiresAt, setExpiresAt] = useState("");
  const expiresAtId = useId();

  const suggestQ = useQuery({
    queryKey: ["share-principals", type, value],
    queryFn: () => listPrincipals(value, type),
    enabled: showSuggest && value.trim().length > 0,
  });

  const suggestions = useMemo(() => suggestQ.data ?? [], [suggestQ.data]);
  const canAdd = value.trim().length > 0 && !pending;

  const submit = () => {
    if (!canAdd) return;
    onAdd(grantBody(type, value.trim(), role, toExpiresAt(expiresAt)));
    setValue("");
    setExpiresAt("");
    setShowSuggest(false);
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
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

        <div className="relative flex-1">
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

        <Select value={role} onValueChange={setRole}>
          <SelectTrigger className="h-9 w-40" aria-label="Role">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {ADD_ROLE_OPTIONS.map((r) => (
              <SelectItem key={r.value} value={r.value}>
                {r.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Button onClick={submit} disabled={!canAdd}>
          {pending ? <Loader2 className="h-4 w-4 animate-spin" /> : "Add"}
        </Button>
      </div>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <label htmlFor={expiresAtId}>Access expires</label>
        <Input
          id={expiresAtId}
          type="date"
          className="h-8 w-40"
          value={expiresAt}
          onChange={(e) => setExpiresAt(e.target.value)}
        />
        <span>optional — leave blank for no expiry</span>
      </div>
      {value.trim().length > 0 && showSuggest && (
        <button
          type="button"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setShowSuggest(false)}
        >
          <X className="h-3 w-3" /> hide suggestions
        </button>
      )}
    </div>
  );
}
