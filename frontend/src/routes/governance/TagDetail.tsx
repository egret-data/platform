import { useEffect, useId, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { AlertTriangle, ArrowLeft, ChevronsUpDown, Lock, Plus, Trash2, X } from "lucide-react";

import { type AccessGroup, type AccessRole, listGroups, listRoles } from "@/api/access";
import {
  type AccessPolicyWritten,
  useClearTagAccessPolicy,
  useDeleteTag,
  useSetTagAccessPolicy,
  useTags,
  useUpdateTag,
} from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import ViewExposureSection from "@/routes/governance/ViewExposureSection";

const DEFAULT_COLOR = "#6366f1";

/**
 * Governance → Classifications → a single tag.
 *
 * A deep-linkable detail page (mirrors the user/role detail pages): identity +
 * key, an auto-saving Details form, and clearance managed as two add-by-search
 * tables — required roles and required groups — rather than a modal stuffed with
 * hundreds of checkboxes. Editing is gated on `platform_admin` / `tag:manage`.
 */
export default function TagDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { me } = useAuth();
  const isAdmin =
    (me?.platform_roles.includes("platform_admin") ?? false) ||
    (me?.permissions?.includes("tag:manage") ?? false);

  const tagsQ = useTags();
  const tag = tagsQ.data?.find((t) => t.id === id);

  const rolesQ = useQuery({ queryKey: ["control-plane", "access", "roles"], queryFn: listRoles, retry: false });
  const groupsQ = useQuery({ queryKey: ["control-plane", "access", "groups"], queryFn: listGroups, retry: false });
  const allRoles: AccessRole[] = rolesQ.data ?? [];
  const allGroups: AccessGroup[] = groupsQ.data ?? [];
  const groupNameById = useMemo(
    () => new Map(allGroups.map((g) => [g.id, g.name])),
    [allGroups],
  );

  const updateTag = useUpdateTag();
  const setAccessPolicy = useSetTagAccessPolicy();
  const clearAccessPolicy = useClearTagAccessPolicy();
  const deleteTag = useDeleteTag();

  // Local editable copies, seeded from the tag once it loads.
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [requiredRoles, setRequiredRoles] = useState<string[]>([]);
  const [requiredGroups, setRequiredGroups] = useState<string[]>([]);
  const [clearanceError, setClearanceError] = useState<string | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);

  useEffect(() => {
    if (tag) {
      setName(tag.name);
      setDescription(tag.description);
      setColor(tag.color || DEFAULT_COLOR);
      setRequiredRoles(tag.required_roles);
      setRequiredGroups(tag.required_groups);
    }
  }, [tag]);

  if (tagsQ.isLoading || rolesQ.isLoading || groupsQ.isLoading) {
    return (
      <div className="space-y-5">
        <BackLink />
        <ListSkeleton rows={6} cols={2} />
      </div>
    );
  }
  if (tagsQ.isError || !tag) {
    return (
      <div className="space-y-5">
        <BackLink />
        <Alert variant="destructive">
          <AlertDescription>{tagsQ.error?.message ?? "Tag not found."}</AlertDescription>
        </Alert>
      </div>
    );
  }

  function saveDetails() {
    if (!tag) return;
    if (name.trim() === tag.name && description.trim() === tag.description && color === tag.color) {
      return;
    }
    updateTag.mutate(
      { id, name: name.trim(), description: description.trim(), color },
      {
        onSuccess: () => toast.success("Tag updated"),
        onError: (e) => toast.error((e as Error).message ?? "Failed to update tag."),
      },
    );
  }

  // Persist the whole clearance set on every add/remove — the API takes the full
  // required_roles/required_groups, and clears the policy when both are empty.
  function persistClearance(roles: string[], groups: string[]) {
    const onError = (e: unknown) => {
      const msg = (e as Error).message ?? "Failed to update clearance.";
      setClearanceError(msg);
      toast.error(msg);
      // Snap back to the server's truth.
      setRequiredRoles(tag?.required_roles ?? []);
      setRequiredGroups(tag?.required_groups ?? []);
    };
    // **SP4c review I5.** The endpoint now says what the accompanying recompile
    // could NOT express, and a success toast that ignored it would be the same
    // "saved, therefore enforced" claim the whole clearance surface exists to
    // avoid: a group-only gate, or a Trino profile with no usable tag service,
    // saves the row and gives no engine anything. The skip is shown where the
    // clearance is authored, not only in the server log.
    const onSuccess = (label: string) => (res: AccessPolicyWritten) => {
      const skipped = res?.skipped ?? [];
      if (skipped.length > 0) {
        const msg = `Saved, but not compiled into any engine: ${skipped
          .map((s) => s.reason)
          .join(" ")}`;
        setClearanceError(msg);
        toast.warning("Clearance saved but not enforced in any engine");
        return;
      }
      setClearanceError(null);
      toast.success(label);
    };
    if (roles.length > 0 || groups.length > 0) {
      setAccessPolicy.mutate(
        { id, required_roles: roles, required_groups: groups },
        { onSuccess: onSuccess("Clearance updated"), onError },
      );
    } else {
      clearAccessPolicy.mutate(id, { onSuccess: onSuccess("Clearance cleared"), onError });
    }
  }

  const addRole = (roleName: string) => {
    const next = [...requiredRoles, roleName];
    setRequiredRoles(next);
    persistClearance(next, requiredGroups);
  };
  const removeRole = (roleName: string) => {
    const next = requiredRoles.filter((r) => r !== roleName);
    setRequiredRoles(next);
    persistClearance(next, requiredGroups);
  };
  const addGroup = (groupId: string) => {
    const next = [...requiredGroups, groupId];
    setRequiredGroups(next);
    persistClearance(requiredRoles, next);
  };
  const removeGroup = (groupId: string) => {
    const next = requiredGroups.filter((g) => g !== groupId);
    setRequiredGroups(next);
    persistClearance(requiredRoles, next);
  };

  const availableRoles = allRoles
    .filter((r) => !requiredRoles.includes(r.name))
    .map((r) => ({ value: r.name, label: r.name }));
  const availableGroups = allGroups
    .filter((g) => !requiredGroups.includes(g.id))
    .map((g) => ({ value: g.id, label: g.name }));
  // One combined list feeding a single clearance table (Type + Name).
  const clearanceRows: ClearanceRow[] = [
    ...requiredRoles.map((n) => ({ type: "role" as const, value: n, label: n })),
    ...requiredGroups.map((gid) => ({
      type: "group" as const,
      value: gid,
      label: groupNameById.get(gid) ?? gid,
    })),
  ];

  // **Only a ROLE compiles to anything in an engine.** A groups-only gate is
  // skipped by the compiler outright (`clearance.rs`: a Ranger item naming a
  // group matches nobody, so a deny emitted for it would withhold the column
  // from everybody), so the three statements below — reads-only, the view
  // bypass, the expansion ceiling — would all be describing a control that does
  // not exist. Claiming withholding where there is none is precisely the
  // failure those statements were written to prevent; the group-only warning
  // above is what a groups-only gate gets, and it says the opposite.
  // Any clearance at all gates OBJECT access through Cedar, which is what the
  // header badge reports.
  const gated = requiredRoles.length > 0 || requiredGroups.length > 0;
  const engineGated = requiredRoles.length > 0;

  return (
    <div className="space-y-6">
      <BackLink />

      {/* Identity header */}
      <div className="flex flex-wrap items-center gap-3">
        <span
          aria-hidden
          className="h-10 w-10 shrink-0 rounded-lg border"
          style={{ backgroundColor: color || DEFAULT_COLOR }}
        />
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="truncate text-2xl font-semibold leading-tight">{tag.name}</h1>
            <Badge variant="secondary" className="font-mono text-xs">
              {tag.key}
            </Badge>
            {gated && (
              <Badge variant="warning" className="gap-1">
                <Lock className="h-3 w-3" />
                Gated
              </Badge>
            )}
          </div>
          <p className="text-sm text-muted-foreground">
            {tag.assignment_count} tagged object{tag.assignment_count === 1 ? "" : "s"}
          </p>
        </div>
        {isAdmin && (
          <div className="ml-auto">
            <Button
              variant="outline"
              size="sm"
              className="text-destructive hover:text-destructive"
              onClick={() => setDeleteOpen(true)}
            >
              <Trash2 className="mr-1 h-4 w-4" />
              Delete
            </Button>
          </div>
        )}
      </div>

      <div className="grid items-start gap-8 lg:grid-cols-2">
      {/* Details */}
      <section className="space-y-3">
        <h2 className="text-base font-semibold">Details</h2>
        <div className="space-y-4">
          {/* Name + color share a row — the primary editable fields up top. */}
          <div className="flex items-end gap-3">
            <div className="flex-1 space-y-1.5">
              <Label htmlFor="tag-name">Name</Label>
              <Input
                id="tag-name"
                value={name}
                disabled={!isAdmin}
                onChange={(e) => setName(e.target.value)}
                onBlur={saveDetails}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="tag-color">Color</Label>
              <Input
                id="tag-color"
                type="color"
                className="h-9 w-12 p-1"
                value={color}
                disabled={!isAdmin}
                onChange={(e) => setColor(e.target.value)}
                onBlur={saveDetails}
              />
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="tag-description">Description</Label>
            <Textarea
              id="tag-description"
              rows={3}
              placeholder="What this tag means…"
              value={description}
              disabled={!isAdmin}
              onChange={(e) => setDescription(e.target.value)}
              onBlur={saveDetails}
            />
          </div>
          {/* Key is immutable — keep it as compact, secondary metadata. */}
          <div className="space-y-1.5">
            <Label htmlFor="tag-key">Key</Label>
            <Input
              id="tag-key"
              value={tag.key}
              readOnly
              disabled
              className="font-mono text-xs"
            />
            <p className="text-xs text-muted-foreground">
              Immutable identifier — set once at creation.
              {isAdmin ? " Other changes save automatically." : ""}
            </p>
          </div>
        </div>
      </section>

      {/* Clearance */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Clearance</h2>
          <p className="text-sm text-muted-foreground">
            Objects tagged this can only be opened by principals holding one of the required roles
            <em> or</em> belonging to one of the required groups. Leave it empty for no restriction.
          </p>
        </div>

        {clearanceError && (
          <Alert variant="destructive">
            <AlertDescription>{clearanceError}</AlertDescription>
          </Alert>
        )}

        {requiredGroups.length > 0 && (
          // SP4c D0/D0b narrow SP4a/SP4b's group finding: it is not only a
          // group name that never reaches Trino or StarRocks — a ROLE name
          // doesn't either. What actually reaches the engine is the
          // enumerated USERNAMES of a required role's members, computed at
          // compile time. A `required_groups` entry has no such expansion, so
          // it is enforced on object access (Cedar) only, never at the query
          // path (SQL Lab, a notebook). Same shape as SP4b's group warning in
          // `PrincipalPicker` — REMOVE THIS once egret ships a group
          // provider, the only thing that would make the underlying fact
          // stale.
          <Badge
            variant="warning"
            className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
          >
            <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
            Not enforced in the engine: no group provider or usersync means a group name never
            reaches Trino or StarRocks, and a role name does not either — only a required role&apos;s
            enumerated members do. A group requirement is enforced on object access only; opening
            the object is gated, but reading the same data through SQL Lab or a notebook is not.
          </Badge>
        )}

        {engineGated && (
          <>
            {/* SP4c Task 7. The earlier wording here claimed Trino withheld
                both reads and writes while StarRocks withheld reads only, and
                called that an asymmetry. The live gate
                (`docs/en/plans/2026-09-28-sp4c-live-result.md`, case 5)
                measured the uncleared subject INSERT *and* DELETE through a
                live clearance on Trino: the compiled deny names a COLUMN
                resource while Trino's write check presents a TABLE one, so
                D1's `insert, delete` never matches. There is no asymmetry —
                neither engine withholds a write. */}
            <Badge
              variant="warning"
              className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
            >
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
              This clearance withholds reads only, on Trino and on StarRocks alike: a user without
              it can still overwrite a column they cannot read. Neither engine withholds writes —
              measured, an uncleared user inserted and deleted through a live clearance on Trino —
              so do not assume a classification blocks writes on either one.
            </Badge>
            {/* View governance Task 12 (C4): the measured behaviour since view
                governance (Task 11 live run). Trino tags a view's columns from
                its tables, StarRocks masks the base column. Trino's DEFINER
                rights still make an uncleared owner's view block everybody. */}
            <Badge
              variant="warning"
              className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
            >
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
              Trino views in egret&apos;s catalogs carry the classification: a user without
              clearance is refused through them. On StarRocks a reader who holds the grant reads
              NULL through a plain or nested view, and an INVOKER view is refused. Existing StarRocks
              materialized views, and views of other connectors, are not covered. On Trino an
              uncleared owner&apos;s DEFINER view blocks everybody, the cleared included.
            </Badge>
            <p className="text-xs text-muted-foreground">
              A StarRocks clearance expands to one policy per tagged column, counted per profile:
              if more than 500 of this tag&apos;s columns are visible to one StarRocks profile,
              that profile&apos;s recompile fails outright rather than truncating the clearance,
              and every write that would recompile it fails with it — until the tag is narrowed or
              untagged back under the limit. Profiles that see fewer are unaffected.
            </p>
          </>
        )}

        <ClearanceTable
          availableRoles={availableRoles}
          availableGroups={availableGroups}
          rows={clearanceRows}
          canEdit={isAdmin}
          onAdd={(type, value) => (type === "role" ? addRole(value) : addGroup(value))}
          onRemove={(type, value) => (type === "role" ? removeRole(value) : removeGroup(value))}
        />
      </section>
      </div>

      <ViewExposureSection tagId={id} engineGated={engineGated} />

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete "${tag.name}"?`}
        description="This tag will be permanently removed from the catalog and from every object it's assigned to."
        confirmText="delete"
        onConfirm={() => {
          deleteTag.mutate(id, {
            onSuccess: () => {
              toast.success("Tag deleted");
              navigate("/governance/classifications");
            },
            onError: (e) => toast.error((e as Error).message ?? "Failed to delete tag."),
          });
          setDeleteOpen(false);
        }}
      />
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/governance/classifications"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Classifications
    </Link>
  );
}

type Option = { value: string; label: string };
type ClearanceType = "role" | "group";
type ClearanceRow = { type: ClearanceType; value: string; label: string };

/**
 * The tag's clearance as ONE table: a Type (role / group) selector + a
 * searchable combobox to pick the item + an explicit Add, over a single table
 * listing every required role and group with its type. Replaces the old split
 * lists / checkbox wall.
 */
function ClearanceTable({
  availableRoles,
  availableGroups,
  rows,
  canEdit,
  onAdd,
  onRemove,
}: {
  availableRoles: Option[];
  availableGroups: Option[];
  rows: ClearanceRow[];
  canEdit: boolean;
  onAdd: (type: ClearanceType, value: string) => void;
  onRemove: (type: ClearanceType, value: string) => void;
}) {
  const [type, setType] = useState<ClearanceType>("role");
  const [open, setOpen] = useState(false);
  const listId = useId();
  const [q, setQ] = useState("");
  const [pending, setPending] = useState<Option | null>(null);

  // Reset the pending pick when the type switches.
  useEffect(() => {
    setPending(null);
    setQ("");
  }, [type]);

  const available = type === "role" ? availableRoles : availableGroups;
  const needle = q.trim().toLowerCase();
  const matches = available
    .filter((o) => needle === "" || o.label.toLowerCase().includes(needle))
    .slice(0, 50);

  return (
    <div className="space-y-3">
      {canEdit && (
        <div className="flex flex-wrap items-center gap-2">
          <Select value={type} onValueChange={(v) => setType(v as ClearanceType)}>
            <SelectTrigger className="w-28" aria-label="Clearance type">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="role">Role</SelectItem>
              <SelectItem value="group">Group</SelectItem>
            </SelectContent>
          </Select>
          <Popover open={open} onOpenChange={setOpen}>
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
                <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-56 p-0" align="start">
              <div className="border-b p-2">
                <Input
                  autoFocus
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
                      className="block w-full px-3 py-1.5 text-left text-sm hover:bg-accent"
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
            <Plus className="mr-1 h-4 w-4" />
            Add
          </Button>
        </div>
      )}

      <div className="rounded-md border">
        <Table>
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
                  Not gated — anyone can open objects with this tag.
                </TableCell>
              </TableRow>
            ) : (
              rows.map((r) => (
                <TableRow key={`${r.type}:${r.value}`}>
                  <TableCell>
                    <Badge variant="outline" className="font-normal capitalize">
                      {r.type}
                    </Badge>
                  </TableCell>
                  <TableCell className="max-w-0 font-medium">
                    <span className="block truncate" title={r.label}>
                      {r.label}
                    </span>
                  </TableCell>
                  <TableCell className="text-right">
                    {canEdit && (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${r.label}`}
                        onClick={() => onRemove(r.type, r.value)}
                      >
                        <X className="h-4 w-4" />
                      </Button>
                    )}
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
