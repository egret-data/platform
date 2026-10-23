import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveRestore, FolderKanban, MoreHorizontal, Pencil, Plus, Trash2 } from "lucide-react";
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
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";
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
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { StatusBadge } from "@/components/ui/status-badge";
import { EntityLink } from "@/components/ui/entity-link";

const SLUG_RE = /^[a-z0-9-]+$/;

/** Derive a URL-safe slug from a display name (lowercase, hyphen-joined). */
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

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
      // Name-first: humans recognise a workspace by its name, so that's the
      // primary link; the slug (a machine id) rides along as a muted sub-line.
      // `value` includes both so search still matches a slug.
      key: "name",
      label: "Workspace",
      sortable: true,
      className: "font-medium",
      value: (w) => `${w.name} ${w.slug}`,
      cell: (w) => (
        <div className="flex flex-col items-start">
          <EntityLink to={`/control-plane/workspaces/${w.id}`}>{w.name}</EntityLink>
          <span className="font-mono text-xs text-muted-foreground">{w.slug}</span>
        </div>
      ),
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
          <StatusBadge tone="success">active</StatusBadge>
        ) : (
          <StatusBadge tone="inactive">archived</StatusBadge>
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
    <PageContainer className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <PageTitle>Workspaces</PageTitle>
          <p className="text-sm text-muted-foreground">
            Every workspace is an independent data platform with its own members and
            resources — create, rename, archive, or open one to manage it.
          </p>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)}>
          <Plus className="mr-1 h-4 w-4" />
          New workspace
        </Button>
      </div>

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
          pageSize={25}
          bordered
          emptyText={
            listQ.isError ? (
              String(listQ.error ?? "Failed to load workspaces.")
            ) : (
              <EmptyState
                icon={FolderKanban}
                title="No workspaces yet"
                description="Create your first workspace to give a team its own data platform."
                action={
                  <Button size="sm" onClick={() => setCreateOpen(true)}>
                    <Plus className="mr-1 h-4 w-4" />
                    New workspace
                  </Button>
                }
              />
            )
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
    </PageContainer>
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
  // Once the admin hand-edits the slug, stop deriving it from the name.
  const [slugEdited, setSlugEdited] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const createMut = useAdminWorkspacesCreateWs();

  useEffect(() => {
    if (open) {
      setSlug("");
      setName("");
      setSlugEdited(false);
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
              <Label htmlFor="new-ws-name">Name</Label>
              <Input
                id="new-ws-name"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  if (!slugEdited) setSlug(slugify(e.target.value));
                }}
                placeholder="Engineering"
                autoFocus
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-ws-slug">Slug</Label>
              <Input
                id="new-ws-slug"
                value={slug}
                onChange={(e) => {
                  setSlugEdited(true);
                  setSlug(e.target.value);
                }}
                placeholder="e.g. engineering"
                className="font-mono"
                required
              />
              {slug.length > 0 && !slugValid ? (
                <p className="text-xs text-destructive">
                  Only lowercase letters, digits, and hyphens.
                </p>
              ) : (
                <p className="text-xs text-muted-foreground">
                  Immutable machine identifier used in URLs and APIs.
                </p>
              )}
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
