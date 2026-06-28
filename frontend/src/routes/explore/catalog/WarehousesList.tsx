import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Boxes, Loader2, Pencil, Plus, Trash2 } from "lucide-react";

import {
  type CreateWarehouseRequest,
  type Warehouse,
  useCreateWarehouse,
  useDeleteWarehouse,
  useRenameWarehouse,
  useWarehouses,
} from "@/api/catalog";
import { ApiError } from "@/api/client";
import { useStorageConns } from "@/api/storage";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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

export default function WarehousesList() {
  const ws = useParams().workspace!;
  const navigate = useNavigate();
  const warehouses = useWarehouses(ws);
  const deleteMutation = useDeleteWarehouse(ws);

  const [addOpen, setAddOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<Warehouse | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Warehouse | null>(null);

  if (warehouses.isLoading) {
    return (
      <section className="p-6">
        <SectionHeader onAdd={() => setAddOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={3} cols={4} />
        </div>
      </section>
    );
  }

  if (warehouses.isError) {
    const status = (warehouses.error as ApiError | null)?.status;
    if (status === 403) {
      return (
        <section className="p-6">
          <Alert>
            <Boxes className="h-4 w-4" />
            <AlertTitle>Insufficient permissions</AlertTitle>
            <AlertDescription>
              Requires workspace-admin to manage catalog warehouses.
            </AlertDescription>
          </Alert>
        </section>
      );
    }
    return (
      <section className="p-6">
        <Alert variant="destructive">
          <AlertTitle>Failed to load warehouses</AlertTitle>
          <AlertDescription>Something went wrong. Please try again.</AlertDescription>
        </Alert>
      </section>
    );
  }

  const rows = warehouses.data ?? [];

  return (
    <section className="p-6">
      <SectionHeader onAdd={() => setAddOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <Boxes className="mx-auto mb-3 h-8 w-8 opacity-60" />
            <p>No warehouses yet — create one on a storage connection.</p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr className="border-b border-border">
                <th className="py-2 pr-4 font-medium">Name</th>
                <th className="py-2 pr-4 font-medium">Storage</th>
                <th className="py-2 pr-4 font-medium">Created</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((wh) => (
                <tr key={wh.id} className="border-b border-border last:border-b-0">
                  <td className="py-2 pr-4 font-medium">
                    <button
                      type="button"
                      onClick={() => navigate(wh.id)}
                      className="text-left hover:underline"
                    >
                      {wh.name}
                    </button>
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    {wh.storage_conn_name ?? (
                      <span className="text-muted-foreground/50">—</span>
                    )}
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    <RelativeTime iso={wh.created_at} />
                  </td>
                  <td className="py-2 text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground"
                        onClick={() => navigate(wh.id)}
                        aria-label={`Open ${wh.name}`}
                      >
                        Open
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground"
                        onClick={() => setRenameTarget(wh)}
                        aria-label={`Rename ${wh.name}`}
                      >
                        <Pencil className="h-4 w-4" />
                        Rename
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() => setDeleteTarget(wh)}
                        aria-label={`Delete ${wh.name}`}
                      >
                        <Trash2 className="h-4 w-4" />
                        Delete
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Add warehouse */}
      <AddWarehouseDialog ws={ws} open={addOpen} onOpenChange={setAddOpen} />

      {/* Rename — key forces remount so the name initializer re-runs per target */}
      <RenameWarehouseDialog
        key={renameTarget?.id ?? "__rename-closed"}
        ws={ws}
        warehouse={renameTarget}
        open={renameTarget !== null}
        onOpenChange={(o) => {
          if (!o) setRenameTarget(null);
        }}
      />

      {/* Delete confirm */}
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete warehouse?"}
        description="This deletes the warehouse and its lakekeeper-provisioned catalog. Tables registered in it will no longer be accessible. This cannot be undone."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </section>
  );
}

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h1 className="text-xl font-semibold">Catalog</h1>
        <p className="text-sm text-muted-foreground">
          Iceberg warehouses for this workspace. Each is provisioned on a storage connection.
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        Add warehouse
      </Button>
    </div>
  );
}

function AddWarehouseDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const conns = useStorageConns(ws);
  const createMutation = useCreateWarehouse(ws);

  const [name, setName] = useState("");
  const [storageConnId, setStorageConnId] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setStorageConnId("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const connOptions = conns.data ?? [];
  const canSubmit =
    name.trim().length > 0 && storageConnId.length > 0 && !createMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);

    const req: CreateWarehouseRequest = {
      name: name.trim(),
      storage_conn_id: storageConnId,
    };

    try {
      await createMutation.mutateAsync(req);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create warehouse.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add warehouse</DialogTitle>
            <DialogDescription>
              Provision an Iceberg warehouse on one of this workspace&apos;s storage connections.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="wh-name">Name</Label>
              <Input
                id="wh-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. analytics"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="wh-storage">Storage connection</Label>
              <Select value={storageConnId} onValueChange={setStorageConnId}>
                <SelectTrigger id="wh-storage" className="w-full">
                  <SelectValue placeholder="Select a storage connection" />
                </SelectTrigger>
                <SelectContent>
                  {connOptions.map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {c.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {connOptions.length === 0 && !conns.isLoading && (
                <p className="text-xs text-muted-foreground">
                  No storage connections yet. Add one under Storage first.
                </p>
              )}
            </div>

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {createMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Add warehouse
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function RenameWarehouseDialog({
  ws,
  warehouse,
  open,
  onOpenChange,
}: {
  ws: string;
  warehouse: Warehouse | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const renameMutation = useRenameWarehouse(ws);
  const [name, setName] = useState(warehouse?.name ?? "");
  const [error, setError] = useState<string | null>(null);

  const canSubmit = name.trim().length > 0 && !renameMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!warehouse || !canSubmit) return;
    setError(null);
    try {
      await renameMutation.mutateAsync({ wid: warehouse.id, name: name.trim() });
      onOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to rename warehouse.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename warehouse</DialogTitle>
            <DialogDescription>Change the display name of this warehouse.</DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="wh-rename">Name</Label>
              <Input
                id="wh-rename"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                autoFocus
              />
            </div>

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {renameMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
