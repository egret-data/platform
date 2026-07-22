import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { ArrowLeft, ChevronsUpDown, Lock, Plus, Trash2, X } from "lucide-react";

import { type AccessGroup, type AccessRole, listGroups, listRoles } from "@/api/access";
import {
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";

const DEFAULT_COLOR = "#6366f1";

/**
 * Control Plane → Data Governance → Classifications → a single tag.
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
      toast.error((e as Error).message ?? "Failed to update clearance.");
      // Snap back to the server's truth.
      setRequiredRoles(tag?.required_roles ?? []);
      setRequiredGroups(tag?.required_groups ?? []);
    };
    if (roles.length > 0 || groups.length > 0) {
      setAccessPolicy.mutate(
        { id, required_roles: roles, required_groups: groups },
        { onSuccess: () => toast.success("Clearance updated"), onError },
      );
    } else {
      clearAccessPolicy.mutate(id, {
        onSuccess: () => toast.success("Clearance cleared"),
        onError,
      });
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
  const selectedRoles = requiredRoles.map((n) => ({ value: n, label: n }));
  const availableGroups = allGroups
    .filter((g) => !requiredGroups.includes(g.id))
    .map((g) => ({ value: g.id, label: g.name }));
  const selectedGroups = requiredGroups.map((gid) => ({
    value: gid,
    label: groupNameById.get(gid) ?? gid,
  }));

  const gated = requiredRoles.length > 0 || requiredGroups.length > 0;

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
        <div className="grid gap-4">
          <div className="space-y-1.5">
            <Label htmlFor="tag-key">Key</Label>
            <Input id="tag-key" value={tag.key} readOnly disabled />
            <p className="text-xs text-muted-foreground">
              The stable identifier — can&apos;t be changed once created.
            </p>
          </div>
          <div className="space-y-1.5">
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
            <Label htmlFor="tag-description">Description</Label>
            <Textarea
              id="tag-description"
              value={description}
              disabled={!isAdmin}
              onChange={(e) => setDescription(e.target.value)}
              onBlur={saveDetails}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="tag-color">Color</Label>
            <Input
              id="tag-color"
              type="color"
              className="h-9 w-16 p-1"
              value={color}
              disabled={!isAdmin}
              onChange={(e) => setColor(e.target.value)}
              onBlur={saveDetails}
            />
          </div>
          {isAdmin && <p className="text-xs text-muted-foreground">Changes save automatically.</p>}
        </div>
      </section>

      {/* Clearance */}
      <section className="space-y-3">
        <div>
          <h2 className="text-base font-semibold">Clearance</h2>
          <p className="text-sm text-muted-foreground">
            Objects tagged this can only be opened by principals holding one of the required roles
            <em> or</em> belonging to one of the required groups. Leave both empty for no
            restriction.
          </p>
        </div>

        <ClearancePicker
          title="Required roles"
          columnHeader="Role"
          itemNoun="role"
          available={availableRoles}
          selected={selectedRoles}
          canEdit={isAdmin}
          onAdd={addRole}
          onRemove={removeRole}
          emptyText="No required roles — not gated by role."
        />
        <ClearancePicker
          title="Required groups"
          columnHeader="Group"
          itemNoun="group"
          available={availableGroups}
          selected={selectedGroups}
          canEdit={isAdmin}
          onAdd={addGroup}
          onRemove={removeGroup}
          emptyText="No required groups — not gated by group."
        />
      </section>
      </div>

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
              navigate("/control-plane/classifications");
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
      to="/control-plane/classifications"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Classifications
    </Link>
  );
}

type Option = { value: string; label: string };

/**
 * One clearance list: a searchable combobox to pick an available item + an
 * explicit Add, then a table of the selected items each with a remove ×. The
 * combobox (search over hundreds of roles/groups) replaces the old checkbox
 * wall.
 */
function ClearancePicker({
  title,
  columnHeader,
  itemNoun,
  available,
  selected,
  canEdit,
  onAdd,
  onRemove,
  emptyText,
}: {
  title: string;
  columnHeader: string;
  itemNoun: string;
  available: Option[];
  selected: Option[];
  canEdit: boolean;
  onAdd: (value: string) => void;
  onRemove: (value: string) => void;
  emptyText: string;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [pending, setPending] = useState<Option | null>(null);

  const needle = q.trim().toLowerCase();
  const matches = available
    .filter((o) => needle === "" || o.label.toLowerCase().includes(needle))
    .slice(0, 50);

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">
        {title}
        {selected.length > 0 && (
          <span className="font-normal text-muted-foreground"> · {selected.length}</span>
        )}
      </h3>
      {canEdit && (
        <div className="flex flex-wrap items-center gap-2">
          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger asChild>
              <Button
                variant="outline"
                role="combobox"
                aria-label={`${columnHeader} to add`}
                className="w-64 justify-between font-normal"
              >
                <span className="truncate">{pending ? pending.label : `Add a ${itemNoun}…`}</span>
                <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-64 p-0" align="start">
              <div className="border-b p-2">
                <Input
                  autoFocus
                  placeholder={`Search ${itemNoun}s…`}
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                />
              </div>
              <div className="max-h-64 overflow-y-auto py-1">
                {matches.length === 0 ? (
                  <p className="px-3 py-2 text-sm text-muted-foreground">No {itemNoun}s found.</p>
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
            aria-label={`Add ${itemNoun}`}
            disabled={!pending}
            onClick={() => {
              if (pending) {
                onAdd(pending.value);
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
              <TableHead>{columnHeader}</TableHead>
              <TableHead className="w-10 text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {selected.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={2} className="py-8 text-center text-muted-foreground">
                  {emptyText}
                </TableCell>
              </TableRow>
            ) : (
              selected.map((s) => (
                <TableRow key={s.value}>
                  <TableCell className="max-w-0 font-medium">
                    <span className="block truncate" title={s.label}>
                      {s.label}
                    </span>
                  </TableCell>
                  <TableCell className="text-right">
                    {canEdit && (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${s.label}`}
                        onClick={() => onRemove(s.value)}
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
