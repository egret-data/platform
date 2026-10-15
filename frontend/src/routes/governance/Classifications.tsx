import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Lock, MoreHorizontal, Pencil, Plus, Trash2 } from "lucide-react";

import { listGroups } from "@/api/access";
import { type Tag, useCreateTag, useDeleteTag, useTags } from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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

const groupsQueryKey = ["control-plane", "access", "groups"] as const;

const DEFAULT_COLOR = "#6366f1";

/** Derive a valid tag key from a name: lowercase, non-`[a-z0-9:_-]` → `-`. */
function slugifyTagKey(s: string): string {
  return s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9:_-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

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
 * Governance → Classifications.
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
  const [deleteTarget, setDeleteTarget] = useState<Tag | null>(null);

  const forbidden = errorStatus(tags.error) === 403;

  const columns: Column<Tag>[] = [
    {
      key: "name",
      label: "Tag",
      sortable: true,
      value: (t) => t.name,
      cell: (t) => (
        <span className="flex items-center gap-2">
          <TagSwatch color={t.color} />
          <Link
            to={`/governance/classifications/${t.id}`}
            className="font-medium text-primary hover:underline"
          >
            {t.name}
          </Link>
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
      // `assignment_count` counts BI objects only, never catalog columns (base
      // or derived from a view's lineage), so the label says so (Task 12, M-6).
      label: "BI objects",
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
              <DropdownMenuItem asChild>
                <Link to={`/governance/classifications/${t.id}`}>
                  <Pencil className="mr-2 h-4 w-4" />
                  Edit
                </Link>
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
                { label: "tagged BI objects", value: totalObjects },
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
  // Once the user edits the key by hand, stop auto-deriving it from the name.
  const [keyEdited, setKeyEdited] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [error, setError] = useState<string | null>(null);

  function onNameChange(v: string) {
    setName(v);
    if (!keyEdited) setKey(slugifyTagKey(v));
  }

  function reset() {
    setKey("");
    setKeyEdited(false);
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

            {/* Name + color share a row — matches the tag detail page. */}
            <div className="flex items-end gap-3">
              <div className="flex-1 space-y-1.5">
                <div className="flex items-center gap-0.5">
                  <Label htmlFor="new-tag-name">Name</Label>
                  <span className="text-destructive" aria-hidden>
                    *
                  </span>
                </div>
                <Input
                  id="new-tag-name"
                  value={name}
                  onChange={(e) => onNameChange(e.target.value)}
                  required
                  autoFocus
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="new-tag-color">Color</Label>
                <Input
                  id="new-tag-color"
                  type="color"
                  className="h-9 w-12 p-1"
                  value={color}
                  onChange={(e) => setColor(e.target.value)}
                />
              </div>
            </div>

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
                onChange={(e) => {
                  setKey(e.target.value);
                  setKeyEdited(true);
                }}
                required
                className="font-mono text-xs"
              />
              <p className="text-xs text-muted-foreground">
                Immutable identifier — auto-filled from the name; edit if you like. Lowercase
                letters, numbers, <code>-</code>, <code>_</code>, <code>:</code>.
              </p>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="new-tag-description">Description</Label>
              <Textarea
                id="new-tag-description"
                rows={3}
                placeholder="What this tag means…"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
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
