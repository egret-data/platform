import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveRestore, MoreHorizontal, Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  getAdminWorkspacesListWsQueryKey,
  useAdminWorkspacesArchiveWs,
  useAdminWorkspacesCreateWs,
  useAdminWorkspacesListWs,
  useAdminWorkspacesRestoreWs,
  useAdminWorkspacesUpdateWs,
} from "@/api/generated/admin-workspaces/admin-workspaces";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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

const SLUG_RE = /^[a-z0-9-]+$/;

/**
 * Control Plane → Workspaces.
 *
 * Platform-admin lifecycle view of every workspace: create, rename, archive,
 * and restore. Each row links to the per-workspace admin cockpit
 * (`/control-plane/workspaces/:id`).
 */
export default function Workspaces() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const listQ = useAdminWorkspacesListWs(undefined, { query: { retry: false } });
  const rows = listQ.data?.status === 200 ? listQ.data.data : [];

  const invalidate = () =>
    qc.invalidateQueries({ queryKey: getAdminWorkspacesListWsQueryKey() });

  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<WorkspaceDto | null>(null);
  const [archiveTarget, setArchiveTarget] = useState<WorkspaceDto | null>(null);
  const [statusFacet, setStatusFacet] = useState(FACET_ALL);

  const archiveMut = useAdminWorkspacesArchiveWs({
    mutation: {
      onSuccess: () => {
        toast.success("Workspace archived");
        invalidate();
      },
      onError: (e) => toast.error(String(e)),
    },
  });
  const restoreMut = useAdminWorkspacesRestoreWs({
    mutation: {
      onSuccess: () => {
        toast.success("Workspace restored");
        invalidate();
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  const total = rows.length;
  const active = rows.filter((w) => w.status === "active").length;
  const filtered = rows.filter(
    (w) => statusFacet === FACET_ALL || w.status === statusFacet,
  );

  const columns: Column<WorkspaceDto>[] = [
    {
      key: "slug",
      label: "Slug",
      sortable: true,
      value: (w) => w.slug,
      cell: (w) => (
        <button
          type="button"
          onClick={() => navigate(`/control-plane/workspaces/${w.id}`)}
          className="font-mono text-sm text-primary hover:underline"
        >
          {w.slug}
        </button>
      ),
    },
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (w) => w.name,
      cell: (w) => w.name,
    },
    {
      key: "owner",
      label: "Owner",
      sortable: true,
      value: (w) => w.owner_name ?? "",
      cell: (w) => (
        <span className="text-sm text-muted-foreground">{w.owner_name ?? "—"}</span>
      ),
    },
    {
      key: "created_at",
      label: "Created",
      sortable: true,
      value: (w) => w.created_at,
      cell: (w) => (
        <span className="text-sm text-muted-foreground" title={fullTimestamp(w.created_at)}>
          {timeAgo(w.created_at)}
        </span>
      ),
    },
    {
      key: "status",
      label: "Status",
      sortable: true,
      value: (w) => w.status,
      cell: (w) =>
        w.status === "active" ? (
          <Badge
            className="border-transparent"
            style={{ backgroundColor: "var(--success)", color: "var(--success-foreground)" }}
          >
            active
          </Badge>
        ) : (
          <Badge variant="secondary">archived</Badge>
        ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (w) => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" aria-label={`Actions for ${w.name}`}>
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onClick={() => navigate(`/control-plane/workspaces/${w.id}`)}>
              <Pencil className="mr-2 h-4 w-4" />
              Open
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => setRenameTarget(w)}>
              <Pencil className="mr-2 h-4 w-4" />
              Rename
            </DropdownMenuItem>
            {w.status === "active" ? (
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => setArchiveTarget(w)}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Archive
              </DropdownMenuItem>
            ) : (
              <DropdownMenuItem onClick={() => restoreMut.mutate({ id: w.id })}>
                <ArchiveRestore className="mr-2 h-4 w-4" />
                Restore
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Workspaces</h1>
        <p className="text-sm text-muted-foreground">
          Every workspace on the platform — create, rename, archive, or open one to manage it.
        </p>
      </div>

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between gap-2">
            <div className="space-y-1.5">
              <CardTitle className="text-lg">All workspaces</CardTitle>
              <CardDescription>
                Each workspace is an independent data platform with its own members and resources.
              </CardDescription>
            </div>
            <Button size="sm" onClick={() => setCreateOpen(true)}>
              <Plus className="mr-1 h-4 w-4" />
              New workspace
            </Button>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <SummaryStrip
            items={[
              { label: "total", value: total },
              { label: "active", value: active },
              { label: "archived", value: total - active },
            ]}
          />
          {listQ.isLoading ? (
            <ListSkeleton rows={6} cols={6} />
          ) : (
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(w) => w.id}
              searchPlaceholder="Search workspaces…"
              initialSort={{ key: "name", dir: "asc" }}
              emptyText={
                listQ.isError
                  ? String(listQ.error ?? "Failed to load workspaces.")
                  : "No workspaces yet."
              }
              filters={
                <FacetSelect
                  label="Status"
                  value={statusFacet}
                  onChange={setStatusFacet}
                  options={[
                    { value: "active", label: "Active" },
                    { value: "archived", label: "Archived" },
                  ]}
                />
              }
            />
          )}
        </CardContent>
      </Card>

      <CreateWorkspaceDialog open={createOpen} onOpenChange={setCreateOpen} onCreated={invalidate} />

      <RenameWorkspaceDialog
        workspace={renameTarget}
        onOpenChange={(o) => {
          if (!o) setRenameTarget(null);
        }}
        onRenamed={invalidate}
      />

      <AlertDialog open={!!archiveTarget} onOpenChange={(o) => !o && setArchiveTarget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Archive workspace?</AlertDialogTitle>
            <AlertDialogDescription>
              <span className="font-medium">{archiveTarget?.name}</span> will be archived and hidden
              from active lookups. You can restore it later.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (archiveTarget) archiveMut.mutate({ id: archiveTarget.id });
                setArchiveTarget(null);
              }}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              Archive
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function CreateWorkspaceDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
}) {
  const [slug, setSlug] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createMut = useAdminWorkspacesCreateWs();

  useEffect(() => {
    if (open) {
      setSlug("");
      setName("");
      setError(null);
    }
  }, [open]);

  const slugValid = SLUG_RE.test(slug);
  const canCreate = slugValid && name.trim().length > 0 && !createMut.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createMut.mutateAsync({ data: { slug: slug.trim(), name: name.trim() } });
      toast.success("Workspace created");
      onCreated();
      onOpenChange(false);
    } catch (err) {
      setError(String(err) || "Failed to create workspace.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New workspace</DialogTitle>
            <DialogDescription>
              A slug (lowercase letters, digits, hyphens) and a display name.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            <div className="space-y-1.5">
              <Label htmlFor="new-ws-slug">Slug</Label>
              <Input
                id="new-ws-slug"
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder="e.g. engineering"
                autoFocus
                required
              />
              {slug.length > 0 && !slugValid && (
                <p className="text-xs text-destructive">
                  Only lowercase letters, digits, and hyphens.
                </p>
              )}
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-ws-name">Name</Label>
              <Input
                id="new-ws-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Engineering"
                required
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
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

function RenameWorkspaceDialog({
  workspace,
  onOpenChange,
  onRenamed,
}: {
  workspace: WorkspaceDto | null;
  onOpenChange: (open: boolean) => void;
  onRenamed: () => void;
}) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const updateMut = useAdminWorkspacesUpdateWs();

  useEffect(() => {
    if (workspace) {
      setName(workspace.name);
      setError(null);
    }
  }, [workspace]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!workspace) return;
    setError(null);
    try {
      await updateMut.mutateAsync({ id: workspace.id, data: { name: name.trim() } });
      toast.success("Workspace renamed");
      onRenamed();
      onOpenChange(false);
    } catch (err) {
      setError(String(err) || "Failed to rename workspace.");
    }
  }

  return (
    <Dialog open={workspace !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename workspace</DialogTitle>
            <DialogDescription>
              Update the display name for <span className="font-medium">{workspace?.slug}</span>.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            <div className="space-y-1.5">
              <Label htmlFor="rename-ws-name">Name</Label>
              <Input
                id="rename-ws-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={updateMut.isPending || name.trim().length === 0}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
