import { useState } from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
import { Loader2, NotebookText, Pencil, Plus, Trash2 } from "lucide-react";

import {
  type Engine,
  type NotebookSummary,
  useCreateNotebook,
  useDeleteNotebook,
  useNotebooks,
  useUpdateNotebook,
} from "@/api/notebooks";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

// ── New notebook dialog ───────────────────────────────────────────────────────

function NewNotebookDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const createMutation = useCreateNotebook(ws);

  const [name, setName] = useState("");
  const [engine, setEngine] = useState<Engine>("daft");

  function handleOpenChange(next: boolean) {
    if (!next) {
      setName("");
      setEngine("daft");
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const created = await createMutation.mutateAsync({
      name: name.trim() || undefined,
      engine,
    });
    handleOpenChange(false);
    navigate(`/explore/${ws}/notebooks/${created.id}`);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Create notebook</DialogTitle>
            <DialogDescription>
              Give your notebook a name and pick an execution engine.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="nb-name">Name</Label>
              <Input
                id="nb-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Untitled"
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="nb-engine">Engine</Label>
              <Select value={engine} onValueChange={(v) => setEngine(v as Engine)}>
                <SelectTrigger id="nb-engine" className="w-full">
                  <SelectValue placeholder="Select an engine" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="daft">Daft</SelectItem>
                  <SelectItem value="spark_connect">Spark Connect</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Rename dialog ─────────────────────────────────────────────────────────────

function RenameNotebookDialog({
  ws,
  notebook,
  open,
  onOpenChange,
}: {
  ws: string;
  notebook: NotebookSummary;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const updateMutation = useUpdateNotebook(ws, notebook.id);
  const [name, setName] = useState(notebook.name);

  function handleOpenChange(next: boolean) {
    if (!next) setName(notebook.name);
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    await updateMutation.mutateAsync({ name: name.trim() });
    handleOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename notebook</DialogTitle>
            <DialogDescription>
              Enter a new name for &laquo;{notebook.name}&raquo;.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-1.5">
            <Label htmlFor="rename-nb-name">Name</Label>
            <Input
              id="rename-nb-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              autoFocus
            />
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={updateMutation.isPending}>
              {updateMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function formatDate(iso: string) {
  try {
    return new Date(iso).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  } catch {
    return iso;
  }
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function Notebooks() {
  const ws = useParams().workspace!;
  const notebooks = useNotebooks(ws);
  const deleteMutation = useDeleteNotebook(ws);

  const [newOpen, setNewOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<NotebookSummary | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<NotebookSummary | null>(null);

  if (notebooks.isLoading) {
    return (
      <section>
        <SectionHeader onAdd={() => setNewOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={3} cols={4} />
        </div>
      </section>
    );
  }

  const rows = notebooks.data ?? [];

  return (
    <section>
      <SectionHeader onAdd={() => setNewOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No notebooks yet.</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {rows.map((nb) => (
              <NotebookCard
                key={nb.id}
                ws={ws}
                notebook={nb}
                onRename={() => setRenameTarget(nb)}
                onDelete={() => setDeleteTarget(nb)}
              />
            ))}
          </div>
        )}
      </div>

      {/* New notebook dialog */}
      <NewNotebookDialog ws={ws} open={newOpen} onOpenChange={setNewOpen} />

      {/* Rename dialog — key forces remount when target changes */}
      {renameTarget && (
        <RenameNotebookDialog
          key={renameTarget.id}
          ws={ws}
          notebook={renameTarget}
          open={renameTarget !== null}
          onOpenChange={(o) => {
            if (!o) setRenameTarget(null);
          }}
        />
      )}

      {/* Delete typed-confirm */}
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete notebook?"}
        description="This notebook and all its cells will be permanently deleted."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </section>
  );
}

// ── Sub-components ────────────────────────────────────────────────────────────

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Notebooks</h2>
        <p className="text-sm text-muted-foreground">
          Python / SQL notebooks backed by the Daft or Spark Connect engine.
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        New notebook
      </Button>
    </div>
  );
}

function NotebookCard({
  ws,
  notebook,
  onRename,
  onDelete,
}: {
  ws: string;
  notebook: NotebookSummary;
  onRename: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="rounded-lg border border-border bg-card p-4 shadow-sm flex flex-col gap-2">
      <div className="flex items-start justify-between gap-2">
        <Link
          to={`/explore/${ws}/notebooks/${notebook.id}`}
          className="flex min-w-0 items-center gap-2 font-medium hover:underline"
        >
          <NotebookText className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="truncate">{notebook.name}</span>
        </Link>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground"
            onClick={onRename}
            aria-label={`Rename ${notebook.name}`}
          >
            <Pencil className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-destructive"
            onClick={onDelete}
            aria-label={`Delete ${notebook.name}`}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <Badge variant="secondary">{notebook.engine}</Badge>
        {notebook.is_public && <Badge variant="outline">Public</Badge>}
      </div>

      <p className="text-xs text-muted-foreground">
        Updated {formatDate(notebook.updated_at)}
      </p>
    </div>
  );
}
