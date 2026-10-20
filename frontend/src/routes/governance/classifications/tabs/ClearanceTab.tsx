import { useId, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { AlertTriangle, ChevronsUpDown, Plus, X } from "lucide-react";
import { type AccessGroup, type AccessRole, listGroups, listRoles } from "@/api/access";
import { ApiError } from "@/api/client";
import { type AccessPolicyWritten, type Tag, useClearTagAccessPolicy, useSetTagAccessPolicy } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TabError } from "@/routes/governance/tabParts";
import { useCanManageTags } from "@/routes/governance/classifications/tagParts";

/** Spec §5.2: what a required role compiles to, stated under the table. */
export const COMPILES_TO =
  "Compiles to one deny tag policy in each Trino profile's tag service, and on StarRocks to a select deny and a NULL mask per tagged column. StarRocks does not access-check a masked column, so an uncleared reader reads NULL there rather than being refused.";

/** Why a picker's list is missing when the server refused it (403): the permission that lists it. */
export const LIST_NEEDS: Record<"role" | "group", string> = {
  role: "Roles are listed with the role:read permission, which this account does not have.",
  group: "Groups are listed with the group:read permission, which this account does not have.",
};

/** Required groups are stored by id; without the group list they can only be shown by id, and the tab says why. */
export const GROUPS_BY_ID =
  "Groups are listed with the group:read permission, which this account does not have, so required groups are shown by id.";

const refused = (e: unknown) => e instanceof ApiError && e.status === 403;

function Warning({ children }: { children: React.ReactNode }) {
  return (
    <Badge variant="warning" className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal">
      <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" aria-hidden />
      {children}
    </Badge>
  );
}

/**
 * The Clearance tab (spec §5.2): TagDetail's clearance editor, moved with its
 * behaviour. One table of required roles and groups; each add or remove
 * writes the whole set (`PUT …/access-policy`, or `DELETE` when it empties);
 * a `skipped` answer is shown here, not reported as saved (SP4c I5); a refusal
 * snaps back to the server's set. The four statements of premise 3 each keep
 * their condition: the group warning with a required group, the other three
 * (and the compile line) with a required role, because a groups-only gate
 * compiles to nothing (`clearance.rs`).
 */
export function ClearanceTab({ tag }: { tag: Tag }) {
  const canEdit = useCanManageTags();
  // The role list only feeds the picker; the group list also names required
  // groups (stored by id). Neither is asked for when nothing on the tab uses it,
  // and a refusal (403) is a missing permission, said as such, not a load error
  // (live check D1: a tag reader saw "could not be loaded" and a Retry).
  const rolesQ = useQuery({
    queryKey: ["control-plane", "access", "roles"],
    queryFn: listRoles,
    retry: false,
    enabled: canEdit,
  });
  const groupsQ = useQuery({
    queryKey: ["control-plane", "access", "groups"],
    queryFn: listGroups,
    retry: false,
    enabled: canEdit || tag.required_groups.length > 0,
  });
  const allRoles: AccessRole[] = useMemo(() => rolesQ.data ?? [], [rolesQ.data]);
  const allGroups: AccessGroup[] = useMemo(() => groupsQ.data ?? [], [groupsQ.data]);
  const groupNameById = useMemo(() => new Map(allGroups.map((g) => [g.id, g.name])), [allGroups]);

  const setAccessPolicy = useSetTagAccessPolicy();
  const clearAccessPolicy = useClearTagAccessPolicy();

  // The editable set, reseeded whenever the server's set changes (adjusted
  // during render rather than in an effect).
  const serverKey = JSON.stringify([tag.required_roles, tag.required_groups]);
  const [seenKey, setSeenKey] = useState(serverKey);
  const [requiredRoles, setRequiredRoles] = useState<string[]>(tag.required_roles);
  const [requiredGroups, setRequiredGroups] = useState<string[]>(tag.required_groups);
  const [clearanceError, setClearanceError] = useState<string | null>(null);
  if (seenKey !== serverKey) {
    setSeenKey(serverKey);
    setRequiredRoles(tag.required_roles);
    setRequiredGroups(tag.required_groups);
  }

  if (rolesQ.isLoading || groupsQ.isLoading) return <ListSkeleton rows={4} cols={2} />;

  function persist(roles: string[], groups: string[]) {
    const onError = (e: unknown) => {
      const msg = (e as Error).message || "The clearance could not be saved. Try again.";
      setClearanceError(msg);
      toast.error(msg);
      setRequiredRoles(tag.required_roles);
      setRequiredGroups(tag.required_groups);
    };
    const onSuccess = (label: string) => (res: AccessPolicyWritten) => {
      const skipped = res?.skipped ?? [];
      if (skipped.length > 0) {
        setClearanceError(`Saved, but not compiled into any engine: ${skipped.map((s) => s.reason).join(" ")}`);
        toast.warning("Clearance saved but not enforced in any engine");
        return;
      }
      setClearanceError(null);
      toast.success(label);
    };
    if (roles.length > 0 || groups.length > 0) {
      setAccessPolicy.mutate(
        { id: tag.id, required_roles: roles, required_groups: groups },
        { onSuccess: onSuccess("Clearance updated"), onError },
      );
    } else {
      clearAccessPolicy.mutate(tag.id, { onSuccess: onSuccess("Clearance cleared"), onError });
    }
  }

  const add = (type: ClearanceType, value: string) => {
    const roles = type === "role" ? [...requiredRoles, value] : requiredRoles;
    const groups = type === "group" ? [...requiredGroups, value] : requiredGroups;
    setRequiredRoles(roles);
    setRequiredGroups(groups);
    persist(roles, groups);
  };
  const remove = (type: ClearanceType, value: string) => {
    const roles = type === "role" ? requiredRoles.filter((r) => r !== value) : requiredRoles;
    const groups = type === "group" ? requiredGroups.filter((g) => g !== value) : requiredGroups;
    setRequiredRoles(roles);
    setRequiredGroups(groups);
    persist(roles, groups);
  };

  const rows: ClearanceRow[] = [
    ...requiredRoles.map((n) => ({ type: "role" as const, value: n, label: n })),
    ...requiredGroups.map((gid) => ({ type: "group" as const, value: gid, label: groupNameById.get(gid) ?? gid })),
  ];
  const engineGated = requiredRoles.length > 0;

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Objects with this classification can only be opened by principals holding one of the required roles{" "}
        <em>or</em> belonging to one of the required groups. Leave it empty for no restriction.
      </p>
      {(rolesQ.isError && !refused(rolesQ.error)) || (groupsQ.isError && !refused(groupsQ.error)) ? (
        <TabError
          what="roles and groups"
          onRetry={() => {
            void rolesQ.refetch();
            void groupsQ.refetch();
          }}
        />
      ) : null}
      {clearanceError ? (
        <Alert variant="destructive">
          <AlertDescription>{clearanceError}</AlertDescription>
        </Alert>
      ) : null}
      {requiredGroups.length > 0 ? (
        // SP4c D0/D0b: a group name never reaches Trino or StarRocks, and a role
        // name does not either; only a required role's members are expanded at
        // compile time. Remove once egret ships a group provider.
        <Warning>
          Not enforced in the engine: no group provider or usersync means a group name never reaches Trino or StarRocks,
          and a role name does not either — only a required role&apos;s enumerated members do. A group requirement is
          enforced on object access only; opening the object is gated, but reading the same data through SQL Lab or a
          notebook is not.
        </Warning>
      ) : null}
      {engineGated ? (
        <>
          {/* SP4c Task 7: measured, neither engine withholds a write. */}
          <Warning>
            This clearance withholds reads only, on Trino and on StarRocks alike: a user without it can still overwrite
            a column they cannot read. Neither engine withholds writes — measured, an uncleared user inserted and
            deleted through a live clearance on Trino — so do not assume a classification blocks writes on either one.
          </Warning>
          {/* View governance Task 12 (C4): measured after view governance. */}
          <Warning>
            Trino views in egret&apos;s catalogs carry the classification: a user without clearance is refused through
            them. On StarRocks a reader who holds the grant reads NULL through a plain or nested view, and an INVOKER
            view is refused. Existing StarRocks materialized views, and views of other connectors, are not covered. On
            Trino an uncleared owner&apos;s DEFINER view blocks everybody, the cleared included.
          </Warning>
          <p className="text-xs text-muted-foreground">
            A StarRocks clearance expands to one policy per tagged column, counted per profile: if more than 500 of this
            tag&apos;s columns are visible to one StarRocks profile, that profile&apos;s recompile fails outright rather
            than truncating the clearance, and every write that would recompile it fails with it — until the tag is
            narrowed or untagged back under the limit. Profiles that see fewer are unaffected.
          </p>
        </>
      ) : null}
      {requiredGroups.length > 0 && refused(groupsQ.error) ? (
        <p className="text-sm text-muted-foreground">{GROUPS_BY_ID}</p>
      ) : null}
      <ClearanceTable
        availableRoles={allRoles.filter((r) => !requiredRoles.includes(r.name)).map((r) => ({ value: r.name, label: r.name }))}
        availableGroups={allGroups.filter((g) => !requiredGroups.includes(g.id)).map((g) => ({ value: g.id, label: g.name }))}
        rows={rows}
        canEdit={canEdit}
        listRefused={{ role: refused(rolesQ.error), group: refused(groupsQ.error) }}
        onAdd={add}
        onRemove={remove}
      />
      {engineGated ? <p className="text-xs text-muted-foreground">{COMPILES_TO}</p> : null}
    </div>
  );
}

type Option = { value: string; label: string };
type ClearanceType = "role" | "group";
type ClearanceRow = { type: ClearanceType; value: string; label: string };

/**
 * The clearance as one table: a Type selector, a searchable picker and Add,
 * over a table of every required role and group. The picker is a Radix
 * popover, which returns focus to its trigger when it closes (#4a D3).
 */
function ClearanceTable({
  availableRoles,
  availableGroups,
  rows,
  canEdit,
  listRefused,
  onAdd,
  onRemove,
}: {
  availableRoles: Option[];
  availableGroups: Option[];
  rows: ClearanceRow[];
  canEdit: boolean;
  /** The server refused this kind's list: the picker says which permission lists it instead. */
  listRefused: Record<ClearanceType, boolean>;
  onAdd: (type: ClearanceType, value: string) => void;
  onRemove: (type: ClearanceType, value: string) => void;
}) {
  const [type, setType] = useState<ClearanceType>("role");
  const [open, setOpen] = useState(false);
  const listId = useId();
  const [q, setQ] = useState("");
  const [pending, setPending] = useState<Option | null>(null);

  const available = type === "role" ? availableRoles : availableGroups;
  const needle = q.trim().toLowerCase();
  const matches = available.filter((o) => needle === "" || o.label.toLowerCase().includes(needle)).slice(0, 50);

  return (
    <div className="space-y-3">
      {canEdit ? (
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={type}
            onValueChange={(v) => {
              setType(v as ClearanceType);
              setPending(null);
              setQ("");
            }}
          >
            <SelectTrigger className="w-28" aria-label="Clearance type">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="role">Role</SelectItem>
              <SelectItem value="group">Group</SelectItem>
            </SelectContent>
          </Select>
          {listRefused[type] ? (
            <p className="text-sm text-muted-foreground">{LIST_NEEDS[type]}</p>
          ) : (
            <>
              <Popover
                open={open}
                onOpenChange={(o) => {
                  setOpen(o);
                  if (!o) setQ("");
                }}
              >
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    role="combobox"
                    aria-expanded={open}
                    aria-controls={listId}
                    aria-label={`${type === "role" ? "Role" : "Group"} to add`}
                    className="w-56 justify-between font-normal"
                  >
                    <span className="truncate">{pending ? pending.label : `Select a ${type}…`}</span>
                    <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" aria-hidden />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-56 p-0" align="start">
                  <div className="border-b p-2">
                    <Input
                      autoFocus
                      aria-label={`Search ${type}s`}
                      placeholder={`Search ${type}s…`}
                      value={q}
                      onChange={(e) => setQ(e.target.value)}
                    />
                  </div>
                  <div id={listId} className="max-h-64 overflow-y-auto py-1">
                    {matches.length === 0 ? (
                      <p className="px-3 py-2 text-sm text-muted-foreground">No {type}s found.</p>
                    ) : (
                      matches.map((o) => (
                        <button
                          key={o.value}
                          type="button"
                          className="block w-full px-3 py-1.5 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
                          onClick={() => {
                            setPending(o);
                            setOpen(false);
                            setQ("");
                          }}
                        >
                          {o.label}
                        </button>
                      ))
                    )}
                  </div>
                </PopoverContent>
              </Popover>
              <Button
                aria-label="Add clearance"
                disabled={!pending}
                onClick={() => {
                  if (pending) {
                    onAdd(type, pending.value);
                    setPending(null);
                  }
                }}
              >
                <Plus className="mr-1 h-4 w-4" aria-hidden />
                Add
              </Button>
              {!pending ? <span className="text-xs text-muted-foreground">Pick a {type} to add.</span> : null}
            </>
          )}
        </div>
      ) : null}
      <div className="rounded-md border">
        <Table aria-label="Required roles and groups">
          <TableHeader>
            <TableRow>
              <TableHead className="w-24">Type</TableHead>
              <TableHead>Name</TableHead>
              <TableHead className="w-10 text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={3} className="py-8 text-center text-muted-foreground">
                  Not gated — anyone can open objects with this classification.
                </TableCell>
              </TableRow>
            ) : (
              rows.map((r) => (
                <TableRow key={`${r.type}:${r.value}`}>
                  <TableCell>
                    <Badge variant="outline" className="font-normal">
                      {r.type}
                    </Badge>
                  </TableCell>
                  <TableCell className="max-w-0 font-medium">
                    <span className="block truncate" title={r.label}>
                      {r.label}
                    </span>
                  </TableCell>
                  <TableCell className="text-right">
                    {canEdit ? (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${r.label}`}
                        onClick={() => onRemove(r.type, r.value)}
                      >
                        <X className="h-4 w-4" aria-hidden />
                      </Button>
                    ) : null}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
