import { useMemo, useState } from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
import { Loader2, Search } from "lucide-react";

import {
  type Engine,
  type NotebookSummary,
  useCreateNotebook,
  useDeleteNotebook,
  useNotebooks,
  useUpdateNotebook,
} from "@/api/notebooks";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
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
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

type SortKey = "name" | "updated_at";

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

// ── Main page ─────────────────────────────────────────────────────────────────

export default function Notebooks() {
  const ws = useParams().workspace!;
  const navigate = useNavigate();
  const { data, isLoading } = useNotebooks(ws);
  const deleteMutation = useDeleteNotebook(ws);

  const [newOpen, setNewOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<NotebookSummary | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<NotebookSummary | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((n) => (!q ? true : n.name.toLowerCase().includes(q)));
    r = [...r].sort((a, b) => {
      const cmp =
        sort.key === "name"
          ? a.name.localeCompare(b.name)
          : a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, query, sort]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }

  const hasData = !!data && data.length > 0;

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Notebooks</h1>
        <Button onClick={() => setNewOpen(true)}>New notebook</Button>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                className="pl-8"
                placeholder="Search notebooks…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} className="pl-6" />
                  <th>Engine</th>
                  <th>Visibility</th>
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((nb) => (
                  <tr key={nb.id} className="border-t hover:bg-accent">
                    <td className="py-1 pl-6">
                      <Link className="font-medium hover:underline" to={`/explore/${ws}/notebooks/${nb.id}`}>
                        {nb.name}
                      </Link>
                    </td>
                    <td>
                      <Badge variant="secondary">{nb.engine}</Badge>
                    </td>
                    <td>
                      {nb.is_public ? (
                        <Badge variant="outline">Public</Badge>
                      ) : (
                        <span className="text-muted-foreground">Private</span>
                      )}
                    </td>
                    <td className="text-muted-foreground">
                      <RelativeTime iso={nb.updated_at} />
                    </td>
                    <td>
                      <RowActionsMenu label={`Actions for ${nb.name}`}>
                        <DropdownMenuItem onClick={() => navigate(`/explore/${ws}/notebooks/${nb.id}`)}>
                          Open
                        </DropdownMenuItem>
                        <DropdownMenuItem onClick={() => setRenameTarget(nb)}>Rename</DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          className="text-destructive focus:text-destructive"
                          onClick={() => setDeleteTarget(nb)}
                        >
                          Delete
                        </DropdownMenuItem>
                      </RowActionsMenu>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">No notebooks match your search.</p>
          )}
        </>
      ) : (
        <p className="text-muted-foreground">No notebooks yet.</p>
      )}

      <NewNotebookDialog ws={ws} open={newOpen} onOpenChange={setNewOpen} />

      {renameTarget && (
        <RenameNotebookDialog
          key={renameTarget.id}
          ws={ws}
          notebook={renameTarget}
          open
          onOpenChange={(o) => {
            if (!o) setRenameTarget(null);
          }}
        />
      )}

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
    </div>
  );
}
