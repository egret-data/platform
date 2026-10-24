import { Fragment, useId, useMemo, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { AlertTriangle, ChevronsUpDown, Info, Plus, X } from "lucide-react";
import { type AccessGroup, type AccessRole, listGroups, listRoles } from "@/api/access";
import { ApiError } from "@/api/client";
import type { ClearedPerson, ClearedVia } from "@/api/generated/model";
import { getPermClearedPeopleQueryKey, usePermClearedPeople } from "@/api/generated/perm/perm";
import { type AccessPolicyWritten, type Tag, useClearTagAccessPolicy, useSetTagAccessPolicy } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TabError } from "@/routes/governance/tabParts";
import { useCanManageTags } from "@/routes/governance/classifications/tagParts";
import { TERM } from "@/routes/governance/glossary";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import type { WorkspaceNames } from "@/routes/governance/permFormat";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { RuleDialog } from "@/routes/governance/RuleDialog";

/** Spec §5.2: what a required role compiles to, stated under the table. Package 2 (D2): StarRocks compiles nothing for a whole-table classification. */
export const COMPILES_TO =
  "Compiles to one deny tag policy in each Trino profile's tag service, and on StarRocks to a select deny and a NULL mask per tagged column. On StarRocks, a classification on a whole table compiles no clearance. StarRocks does not access-check a masked column, so an uncleared reader reads NULL there rather than being refused.";

/** Why a picker's list is missing when the server refused it (403): the permission that lists it. */
export const LIST_NEEDS: Record<"role" | "group", string> = {
  role: "Roles are listed with the role:read permission, which this account does not have.",
  group: "Groups are listed with the group:read permission, which this account does not have.",
};

/** Required groups are stored by id; without the group list they can only be shown by id, and the tab says why. */
export const GROUPS_BY_ID =
  "Groups are listed with the group:read permission, which this account does not have, so required groups are shown by id.";

const refused = (e: unknown) => e instanceof ApiError && e.status === 403;

/** Spec C9.1: what clearance does, stated before anything is added. */
export const CLEARANCE_SCOPE = {
  lead: "Clearance decides who may open data with this classification",
  where: " in egret (tables, columns, BI objects).",
  engines:
    "On engines it is enforced only through required roles: their members are named in each engine's policy. A required group is enforced when the data is opened in egret, not in SQL Lab or notebooks.",
  maskBefore: "To hide values from everyone without clearance, ",
  maskAction: "add a mask rule",
  maskAfter: " as well.",
} as const;

/** The empty clearance (glossary): no requirement beyond the data's own access. */
export const NO_CLEARANCE = "None: anyone who can read the data can open it.";
export const CLEARED_NEEDS_POLICY_READ = "Seeing who is cleared needs the policy:read permission.";
export const ADMINS_ALWAYS = "Platform admins can always open it in egret.";
export const ENGINES_DO_NOT_ENFORCE = "Engines do not enforce this clearance: anyone with SQL access can read the data.";
/** A person egret clears whom the engines' deny does not except (spec C9 table). */
export const REFUSED_ON_ENGINES = "Cleared in egret, refused on engines";
export const NO_ONE_LISTED = "No one holds a required role or belongs to a required group yet.";
export const ADD_MASK_NEEDS_POLICY_UPDATE = "Adding a mask rule needs the policy:update permission.";
/**
 * What the "engines" marker means, said where it appears: `onEngines` says the
 * compiled clearance deny excepts this person, nothing about any grant.
 */
const ENGINES_MARK_TITLE = "Excepted from the engines' clearance deny (reading still needs a grant).";

/** A neutral statement (the clearance's scope), muted with an Info icon; warnings use `Callout`. */
function ScopeNote({ children }: { children: ReactNode }) {
  return (
    <div role="note" className="flex gap-2 rounded-md border bg-muted/50 p-3 text-sm text-foreground">
      <Info className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
      <div className="min-w-0 space-y-1">{children}</div>
    </div>
  );
}

/**
 * A measured warning, drawn with tokens only (spec C9.3,
 * N21): a 15% tint of `--highlight` behind `--foreground` text, which keeps
 * the foreground's contrast. `Badge variant="warning"` (a raw palette) is not
 * used here.
 */
function Callout({ children }: { children: ReactNode }) {
  return (
    <div role="note" className="flex gap-2 rounded-md border border-highlight bg-highlight/15 p-3 text-sm text-foreground">
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-foreground" aria-hidden />
      <div className="min-w-0 space-y-1">{children}</div>
    </div>
  );
}

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

/** One way a person is listed: `role cleared in workspace Sales only through group ops`. */
function viaText(v: ClearedVia, workspaceName: WorkspaceNames): string {
  let text = `${v.kind} ${v.name}`;
  if (v.scope === "workspace") {
    text += v.scopeId ? ` in workspace ${workspaceName(v.scopeId) ?? v.scopeId} only` : " in one workspace only";
  }
  else if (v.scope === "object") text += " on one object only";
  if (v.throughGroup) text += ` through group ${v.throughGroup}`;
  return text;
}

/**
 * `label · username · ways · markers` (spec C9.2). "engines" marks a person
 * the engines' deny excepts; when engines enforce the clearance, everyone
 * else listed is cleared in egret and refused on engines (a group member, a
 * role held through a group, a holder without a username).
 */
function Person({ p, enginesEnforce, workspaceName }: { p: ClearedPerson; enginesEnforce: boolean; workspaceName: WorkspaceNames }) {
  const parts: ReactNode[] = [p.label];
  if (p.username) parts.push(p.username);
  if (p.via.length > 0) parts.push(p.via.map((v) => viaText(v, workspaceName)).join(", "));
  if (p.serviceAccount) parts.push("service account");
  if (p.status === "disabled") parts.push("disabled");
  if (p.onEngines)
    parts.push(
      <abbr key="engines" title={ENGINES_MARK_TITLE} className="no-underline">
        engines
      </abbr>,
    );
  if (enginesEnforce && !p.onEngines) parts.push(REFUSED_ON_ENGINES);
  return (
    <li className="break-words">
      {parts.map((part, i) => (
        <Fragment key={i}>
          {i > 0 ? " · " : null}
          {part}
        </Fragment>
      ))}
    </li>
  );
}

/** The listed people; mounted only with an answer, so workspace names are asked for only then. */
function ClearedList({
  people,
  count,
  truncated,
  enginesEnforce,
}: {
  people: ClearedPerson[];
  count: number;
  truncated: boolean;
  enginesEnforce: boolean;
}) {
  const workspaceName = useWorkspaceNames();
  return (
    <>
      <ul aria-label="Cleared people" className="mt-2 space-y-0.5 text-sm">
        {people.map((p) => (
          <Person key={p.userId} p={p} enginesEnforce={enginesEnforce} workspaceName={workspaceName} />
        ))}
      </ul>
      {truncated ? <p className="text-xs text-muted-foreground">{`and ${count - people.length} more`}</p> : null}
    </>
  );
}

/**
 * Who is cleared now (spec C9.2, `GET /perm/clearances/{tag}/people`,
 * `policy:read`). The count is the people the clearance's roles and groups
 * list, which is neither egret's cleared set (platform admins are not listed)
 * nor the engines' (a group never reaches them), so it is never called "can
 * open it". Without `policy:read` the section says what reading it needs.
 */
function ClearedPeople({ tag }: { tag: Tag }) {
  const canRead = usePolicyPermissions()("policy:read");
  const roles = tag.required_roles.length;
  const groups = tag.required_groups.length;
  const peopleQ = usePermClearedPeople(tag.id, { query: { retry: false, enabled: canRead } });
  // Only a settled answer speaks: during the refetch a clearance write starts,
  // the old answer would describe the old clearance.
  const answer = peopleQ.data?.status === 200 && !peopleQ.isFetching ? peopleQ.data.data : null;
  // A clearance that names no role compiles to nothing on engines (`enginesEnforce`);
  // the server's tag says the same. Unknown (null) while the answer is pending.
  const enginesEnforce: boolean | null = !canRead ? roles > 0 : answer ? answer.enginesEnforce : null;

  let list: ReactNode;
  if (!canRead) {
    list = (
      <>
        <p className="text-sm tabular-nums">
          {`${roles} ${plural(roles, "role", "roles")} and ${groups} ${plural(groups, "group", "groups")} required.`}
        </p>
        <p className="text-sm text-muted-foreground">{CLEARED_NEEDS_POLICY_READ}</p>
      </>
    );
  } else if (peopleQ.isError && !peopleQ.isFetching) {
    list = <TabError what="cleared people" onRetry={() => void peopleQ.refetch()} />;
  } else if (!answer) {
    list = <Skeleton className="h-4 w-64" />;
  } else if (answer.count === 0) {
    list = <p className="text-sm">{NO_ONE_LISTED}</p>;
  } else {
    list = (
      <details>
        <summary className="cursor-pointer text-sm font-medium tabular-nums">
          {answer.count === 1
            ? "1 person holds a required role or belongs to a required group"
            : `${answer.count} people hold a required role or belong to a required group`}
        </summary>
        <ClearedList
          people={answer.people}
          count={answer.count}
          truncated={answer.truncated}
          enginesEnforce={answer.enginesEnforce}
        />
        {answer.enginesEnforce ? (
          <p className="mt-1 text-xs text-muted-foreground tabular-nums">
            {`${answer.onEnginesCount} of them ${plural(answer.onEnginesCount, "is", "are")} named in the engines' policy (marked “engines”).`}
          </p>
        ) : null}
      </details>
    );
  }

  return (
    <section aria-label="Who is cleared" className="space-y-1">
      {list}
      <p className="text-sm text-muted-foreground">{ADMINS_ALWAYS}</p>
      {enginesEnforce === false ? <p className="text-sm">{ENGINES_DO_NOT_ENFORCE}</p> : null}
    </section>
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
  const queryClient = useQueryClient();
  const canAddRule = usePolicyPermissions()("policy:update");
  const [addingMask, setAddingMask] = useState(false);

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
      // Who the clearance lists changed with it (spec C9.2).
      void queryClient.invalidateQueries({ queryKey: getPermClearedPeopleQueryKey(tag.id) });
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

  const hasClearance = requiredRoles.length + requiredGroups.length > 0;
  const serverHasClearance = tag.required_roles.length + tag.required_groups.length > 0;
  // The editable set differs from the server's: a write is in flight (or just failed and is snapping back).
  // Compared as sets: the server may answer the same set in another order.
  const asSet = (roles: string[], groups: string[]) => JSON.stringify([[...roles].sort(), [...groups].sort()]);
  const saving = asSet(requiredRoles, requiredGroups) !== asSet(tag.required_roles, tag.required_groups);

  return (
    <div className="space-y-3">
      <ScopeNote>
        <p>
          <strong>{CLEARANCE_SCOPE.lead}</strong>
          {CLEARANCE_SCOPE.where}
        </p>
        <p>{CLEARANCE_SCOPE.engines}</p>
        <p>
          {CLEARANCE_SCOPE.maskBefore}
          {canAddRule ? (
            <Button variant="link" className="h-auto p-0 align-baseline" onClick={() => setAddingMask(true)}>
              {CLEARANCE_SCOPE.maskAction}
            </Button>
          ) : (
            CLEARANCE_SCOPE.maskAction
          )}
          {CLEARANCE_SCOPE.maskAfter}
        </p>
        {canAddRule ? null : <p className="text-xs text-muted-foreground">{ADD_MASK_NEEDS_POLICY_UPDATE}</p>}
      </ScopeNote>
      {canAddRule ? (
        <RuleDialog
          open={addingMask}
          onOpenChange={setAddingMask}
          entry={{ kind: "classification", tagKey: tag.key, tagName: tag.name }}
        />
      ) : null}
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
      {hasClearance ? (
        <details>
          <summary className="cursor-pointer text-sm font-medium">Engine details</summary>
          <div className="mt-2 space-y-2">
            {requiredGroups.length > 0 ? (
              // SP4c D0/D0b: a group name never reaches Trino or StarRocks, and a role
              // name does not either; only a required role's members are expanded at
              // compile time. Remove once egret ships a group provider.
              <Callout>
                Not enforced in the engine: no group provider or usersync means a group name never reaches Trino or StarRocks,
                and a role name does not either — only a required role&apos;s enumerated members do. A group requirement is
                enforced on object access only; opening the object is gated, but reading the same data through SQL Lab or a
                notebook is not.
              </Callout>
            ) : null}
            {engineGated ? (
              <>
                {/* SP4c Task 7: measured, neither engine withholds a write. */}
                <Callout>
                  This clearance withholds reads only, on Trino and on StarRocks alike: a user without it can still overwrite
                  a column they cannot read. Neither engine withholds writes — measured, an uncleared user inserted and
                  deleted through a live clearance on Trino — so do not assume a classification blocks writes on either one.
                </Callout>
                {/* View governance Task 12 (C4): measured after view governance. */}
                <Callout>
                  Trino views in egret&apos;s catalogs carry the classification: a user without clearance is refused through
                  them. On StarRocks a reader who holds the grant reads NULL through a plain or nested view, and an INVOKER
                  view is refused. Existing StarRocks materialized views, and views of other connectors, are not covered. On
                  Trino an uncleared owner&apos;s DEFINER view blocks everybody, the cleared included.
                </Callout>
                <Callout>
                  A StarRocks clearance expands to one policy per tagged column, counted per profile: if more than 500 of this
                  tag&apos;s columns are visible to one StarRocks profile, that profile&apos;s recompile fails outright rather
                  than truncating the clearance, and every write that would recompile it fails with it — until the tag is
                  narrowed or untagged back under the limit. Profiles that see fewer are unaffected.
                </Callout>
              </>
            ) : null}
          </div>
        </details>
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
      {/* From the server's tag only, and not while a write is in flight, so no sentence describes a set that is not saved. */}
      {saving ? null : serverHasClearance ? (
        <ClearedPeople tag={tag} />
      ) : (
        <p className="text-sm text-muted-foreground">{ADMINS_ALWAYS}</p>
      )}
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
  const addReasonId = useId();
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
                // Focusable while blocked, with its reason tied (N22, FE-ACTION-4); a click then does nothing.
                aria-disabled={!pending ? true : undefined}
                aria-describedby={!pending ? addReasonId : undefined}
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
              {!pending ? (
                <span id={addReasonId} className="text-xs text-muted-foreground">
                  Pick a {type} to add.
                </span>
              ) : null}
            </>
          )}
        </div>
      ) : null}
      <div className="rounded-md border">
        <Table aria-label={TERM.clearance}>
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
                  {NO_CLEARANCE}
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
