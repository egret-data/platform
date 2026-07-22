import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Lock, MoreHorizontal, Pencil, Plus, Trash2 } from "lucide-react";

import { listGroups, listRoles } from "@/api/access";
import {
  type Tag,
  useClearTagAccessPolicy,
  useCreateTag,
  useDeleteTag,
  useSetTagAccessPolicy,
  useTags,
  useUpdateTag,
} from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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
import { Textarea } from "@/components/ui/textarea";
import { errorStatus } from "@/lib/httpStatus";

const rolesQueryKey = ["control-plane", "access", "roles"] as const;
const groupsQueryKey = ["control-plane", "access", "groups"] as const;

const DEFAULT_COLOR = "#6366f1";

/** Small rounded color swatch used next to a tag's name. */
function TagSwatch({ color }: { color: string }) {
  return (
    <span
      aria-hidden="true"
      className="inline-block h-3 w-3 shrink-0 rounded-full border border-border"
      style={{ backgroundColor: color || DEFAULT_COLOR }}
    />
  );
}

/**
 * Control Plane → Data Governance → Classifications.
 *
 * Lists the org-wide catalog of classification tags used to classify data
 * and BI objects. Create/edit/delete are gated on `platform_admin` or the
 * `tag:manage` permission; everyone with `tag:read` (route-gated in App.tsx)
 * sees the read-only table.
 */
export default function Classifications() {
  const { me } = useAuth();
  const isAdmin =
    (me?.platform_roles.includes("platform_admin") ?? false) ||
    (me?.permissions?.includes("tag:manage") ?? false);

  const tags = useTags();
  const rows = tags.data ?? [];
  const deleteTag = useDeleteTag();

  // Fetched here (not just inside the editor) so the table's Gated badge can
  // resolve `required_groups` UUIDs to names.
  const groups = useQuery({
    queryKey: groupsQueryKey,
    queryFn: listGroups,
    retry: false,
  });
  const groupNameById = useMemo(
    () => new Map((groups.data ?? []).map((g) => [g.id, g.name])),
    [groups.data],
  );

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<Tag | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Tag | null>(null);

  const forbidden = errorStatus(tags.error) === 403;

  const columns: Column<Tag>[] = [
    {
      key: "name",
      label: "Tag",
      sortable: true,
      className: "font-medium",
      value: (t) => t.name,
      cell: (t) => (
        <span className="flex items-center gap-2">
          <TagSwatch color={t.color} />
          {t.name}
        </span>
      ),
    },
    {
      key: "key",
      label: "Key",
      sortable: true,
      value: (t) => t.key,
      cell: (t) => <span className="font-mono text-xs">{t.key}</span>,
    },
    {
      key: "description",
      label: "Description",
      value: (t) => t.description,
      cell: (t) => (
        <span className="text-muted-foreground">{t.description || "—"}</span>
      ),
    },
    {
      key: "objects",
      label: "Objects",
      sortable: true,
      value: (t) => t.assignment_count,
      cell: (t) => <Badge variant="secondary">{t.assignment_count}</Badge>,
    },
    {
      key: "clearance",
      label: "Clearance",
      sortable: true,
      value: (t) => t.required_roles.length + t.required_groups.length,
      // A tag can be gated by many roles/groups, so show a compact count with
      // the full list on hover rather than a badge that grows without bound.
      cell: (t) => {
        const nr = t.required_roles.length;
        const ng = t.required_groups.length;
        if (!nr && !ng) return <span className="text-muted-foreground">—</span>;
        const parts: string[] = [];
        if (nr) parts.push(t.required_roles.join(", "));
        if (ng) {
          const names = t.required_groups.map((id) => groupNameById.get(id) ?? id);
          parts.push(`groups: ${names.join(", ")}`);
        }
        const count = [
          nr ? `${nr} role${nr > 1 ? "s" : ""}` : null,
          ng ? `${ng} group${ng > 1 ? "s" : ""}` : null,
        ]
          .filter(Boolean)
          .join(", ");
        return (
          <Badge variant="warning" className="gap-1" title={parts.join(" + ")}>
            <Lock className="h-3 w-3" />
            Gated · {count}
          </Badge>
        );
      },
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (t) =>
        isAdmin && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={`Actions for ${t.name}`}>
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => setEditTarget(t)}>
                <Pencil className="mr-2 h-4 w-4" />
                Edit
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => setDeleteTarget(t)}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ),
    },
  ];

  const gatedCount = rows.filter(
    (t) => t.required_roles.length > 0 || t.required_groups.length > 0,
  ).length;
  const totalObjects = rows.reduce((n, t) => n + t.assignment_count, 0);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold leading-tight">Classifications</h1>
          <p className="text-sm text-muted-foreground">
            Define the tags used to classify data and BI objects.
          </p>
        </div>
        {isAdmin && (
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus className="mr-1 h-4 w-4" />
            New tag
          </Button>
        )}
      </div>

      <div className="space-y-4">
        {tags.isLoading ? (
          <ListSkeleton rows={6} cols={5} />
        ) : forbidden ? (
          <Alert variant="destructive">
            <AlertDescription>
              You need the <code>tag:read</code> permission.
            </AlertDescription>
          </Alert>
        ) : tags.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {(tags.error as Error)?.message ?? "Failed to load tags."}
            </AlertDescription>
          </Alert>
        ) : (
          <>
            <SummaryStrip
              items={[
                { label: "tags", value: rows.length },
                { label: "gated", value: gatedCount },
                { label: "tagged objects", value: totalObjects },
              ]}
            />
            <FilterableTable
              rows={rows}
              columns={columns}
              getId={(t) => t.id}
              searchPlaceholder="Search tags…"
              initialSort={{ key: "name", dir: "asc" }}
              pageSize={25}
              bordered
              emptyText="No tags yet."
            />
          </>
        )}
      </div>

      <CreateTagDialog open={createOpen} onOpenChange={setCreateOpen} />
      <EditTagDialog
        tag={editTarget}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete tag?"}
        description="This tag will be permanently removed from the catalog and from every object it's assigned to."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteTag.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}

function CreateTagDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const createTag = useCreateTag();
  const [key, setKey] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setKey("");
    setName("");
    setDescription("");
    setColor(DEFAULT_COLOR);
    setError(null);
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createTag.mutateAsync({
        key: key.trim(),
        name: name.trim(),
        description: description.trim(),
        color,
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as Error).message ?? "Failed to create tag.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New tag</DialogTitle>
            <DialogDescription>
              Create a classification tag that can be assigned to data and BI objects.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <div className="flex items-center gap-0.5">
                <Label htmlFor="new-tag-key">Key</Label>
                <span className="text-destructive" aria-hidden>
                  *
                </span>
              </div>
              <Input
                id="new-tag-key"
                value={key}
                onChange={(e) => setKey(e.target.value)}
                required
                autoFocus
              />
              <p className="text-xs text-muted-foreground">
                The stable identifier — can&apos;t be changed once created.
              </p>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center gap-0.5">
                <Label htmlFor="new-tag-name">Name</Label>
                <span className="text-destructive" aria-hidden>
                  *
                </span>
              </div>
              <Input
                id="new-tag-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-tag-description">Description</Label>
              <Textarea
                id="new-tag-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-tag-color">Color</Label>
              <Input
                id="new-tag-color"
                type="color"
                className="h-9 w-16 p-1"
                value={color}
                onChange={(e) => setColor(e.target.value)}
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createTag.isPending || !key.trim() || !name.trim()}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function EditTagDialog({
  tag,
  onOpenChange,
}: {
  tag: Tag | null;
  onOpenChange: (open: boolean) => void;
}) {
  const updateTag = useUpdateTag();
  const setAccessPolicy = useSetTagAccessPolicy();
  const clearAccessPolicy = useClearTagAccessPolicy();
  const roles = useQuery({
    queryKey: rolesQueryKey,
    queryFn: listRoles,
    retry: false,
    enabled: tag !== null,
  });
  const groups = useQuery({
    queryKey: groupsQueryKey,
    queryFn: listGroups,
    retry: false,
    enabled: tag !== null,
  });
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [requiredRoles, setRequiredRoles] = useState<string[]>([]);
  const [requiredGroups, setRequiredGroups] = useState<string[]>([]);
  const [roleFilter, setRoleFilter] = useState("");
  const [groupFilter, setGroupFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (tag) {
      setName(tag.name);
      setDescription(tag.description);
      setColor(tag.color || DEFAULT_COLOR);
      setRequiredRoles(tag.required_roles);
      setRequiredGroups(tag.required_groups);
      setRoleFilter("");
      setGroupFilter("");
      setError(null);
    }
  }, [tag]);

  const roleMatches = (roles.data ?? []).filter((r) =>
    r.name.toLowerCase().includes(roleFilter.trim().toLowerCase()),
  );
  const groupMatches = (groups.data ?? []).filter((g) =>
    g.name.toLowerCase().includes(groupFilter.trim().toLowerCase()),
  );

  function handleOpenChange(next: boolean) {
    if (!next) setError(null);
    onOpenChange(next);
  }

  function toggleRole(roleName: string, checked: boolean) {
    setRequiredRoles((prev) =>
      checked ? [...prev, roleName] : prev.filter((r) => r !== roleName),
    );
  }

  function toggleGroup(groupId: string, checked: boolean) {
    setRequiredGroups((prev) =>
      checked ? [...prev, groupId] : prev.filter((g) => g !== groupId),
    );
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!tag) return;
    setError(null);
    try {
      await updateTag.mutateAsync({
        id: tag.id,
        name: name.trim(),
        description: description.trim(),
        color,
      });
      if (requiredRoles.length > 0 || requiredGroups.length > 0) {
        await setAccessPolicy.mutateAsync({
          id: tag.id,
          required_roles: requiredRoles,
          required_groups: requiredGroups,
        });
      } else if (tag.required_roles.length > 0 || tag.required_groups.length > 0) {
        await clearAccessPolicy.mutateAsync(tag.id);
      }
      handleOpenChange(false);
    } catch (err) {
      setError((err as Error).message ?? "Failed to save tag.");
    }
  }

  const saving = updateTag.isPending || setAccessPolicy.isPending || clearAccessPolicy.isPending;

  return (
    <Dialog open={tag !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Edit tag — {tag?.name}</DialogTitle>
            <DialogDescription>
              The key is immutable once a tag is created.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="edit-tag-key">Key</Label>
              <Input id="edit-tag-key" value={tag?.key ?? ""} readOnly disabled />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="edit-tag-name">Name</Label>
              <Input
                id="edit-tag-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="edit-tag-description">Description</Label>
              <Textarea
                id="edit-tag-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="edit-tag-color">Color</Label>
              <Input
                id="edit-tag-color"
                type="color"
                className="h-9 w-16 p-1"
                value={color}
                onChange={(e) => setColor(e.target.value)}
              />
            </div>

            <div className="space-y-1.5">
              <Label>
                Required roles (clearance){" "}
                {requiredRoles.length > 0 && (
                  <span className="font-normal text-muted-foreground">
                    · {requiredRoles.length} selected
                  </span>
                )}
              </Label>
              {roles.data && roles.data.length > 8 && (
                <Input
                  className="h-8"
                  placeholder="Filter roles…"
                  value={roleFilter}
                  onChange={(e) => setRoleFilter(e.target.value)}
                />
              )}
              <div className="max-h-40 space-y-2 overflow-y-auto rounded-md border p-3">
                {roles.isLoading ? (
                  <p className="text-sm text-muted-foreground">Loading roles…</p>
                ) : roleMatches.length > 0 ? (
                  roleMatches.map((r) => {
                    const id = `edit-tag-clearance-${r.id}`;
                    return (
                      <div key={r.id} className="flex items-center gap-2">
                        <Checkbox
                          id={id}
                          checked={requiredRoles.includes(r.name)}
                          onCheckedChange={(c) => toggleRole(r.name, c === true)}
                        />
                        <Label htmlFor={id} className="cursor-pointer font-normal">
                          {r.name}
                        </Label>
                      </div>
                    );
                  })
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {roles.data && roles.data.length > 0 ? "No roles match." : "No roles defined."}
                  </p>
                )}
              </div>
              <p className="text-xs text-muted-foreground">
                Objects tagged this can only be opened by principals holding one of
                these roles. Empty = no restriction.
              </p>
            </div>

            <div className="space-y-1.5">
              <Label>
                Required groups (clearance){" "}
                {requiredGroups.length > 0 && (
                  <span className="font-normal text-muted-foreground">
                    · {requiredGroups.length} selected
                  </span>
                )}
              </Label>
              {groups.data && groups.data.length > 8 && (
                <Input
                  className="h-8"
                  placeholder="Filter groups…"
                  value={groupFilter}
                  onChange={(e) => setGroupFilter(e.target.value)}
                />
              )}
              <div className="max-h-40 space-y-2 overflow-y-auto rounded-md border p-3">
                {groups.isLoading ? (
                  <p className="text-sm text-muted-foreground">Loading groups…</p>
                ) : groupMatches.length > 0 ? (
                  groupMatches.map((g) => {
                    const id = `edit-tag-clearance-group-${g.id}`;
                    return (
                      <div key={g.id} className="flex items-center gap-2">
                        <Checkbox
                          id={id}
                          checked={requiredGroups.includes(g.id)}
                          onCheckedChange={(c) => toggleGroup(g.id, c === true)}
                        />
                        <Label htmlFor={id} className="cursor-pointer font-normal">
                          {g.name}
                        </Label>
                      </div>
                    );
                  })
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {groups.data && groups.data.length > 0
                      ? "No groups match."
                      : "No groups defined."}
                  </p>
                )}
              </div>
              <p className="text-xs text-muted-foreground">
                …or belonging to one of these groups. Empty = no restriction from
                groups.
              </p>
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={saving}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
