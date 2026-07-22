import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, Loader2, Plus, Trash2 } from "lucide-react";

import {
  type Warehouse,
  type CreateWarehouseInput,
  createWarehouse,
  deleteWarehouse,
  listStorage,
  listWarehouses,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { EmptyState } from "@/components/explore/EmptyState";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  FilterableTable,
  type Column,
} from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

export const warehousesQueryKey = ["control-plane", "warehouses"] as const;

/**
 * Column set for the warehouses table. `value` (used for search + sort) resolves
 * the workspace/storage ids to their display names so a search matches what the
 * row actually shows, and the provisioned column returns its badge label so
 * "provisioned"/"pending" are searchable too.
 */
function warehouseColumns(
  wsName: (id: string) => string,
  storageName: (id: string) => string,
  onDelete: (wh: Warehouse) => void,
): Column<Warehouse>[] {
  return [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (wh) => wh.name,
      cell: (wh) => (
        <Link
          to={`/control-plane/warehouses/${wh.id}`}
          className="font-medium text-primary hover:underline"
        >
          {wh.name}
        </Link>
      ),
    },
    {
      key: "workspace",
      label: "Workspace",
      sortable: true,
      value: (wh) => wsName(wh.workspaceId),
      cell: (wh) => <span className="text-muted-foreground">{wsName(wh.workspaceId)}</span>,
    },
    {
      key: "storage",
      label: "Storage",
      sortable: true,
      value: (wh) => storageName(wh.storageConnId),
      cell: (wh) => <span className="text-muted-foreground">{storageName(wh.storageConnId)}</span>,
    },
    {
      key: "provisioned",
      label: "Provisioned",
      sortable: true,
      value: (wh) => (wh.lakekeeperWarehouseId ? "Provisioned" : "Pending"),
      cell: (wh) => (
        <Badge variant={wh.lakekeeperWarehouseId ? "secondary" : "outline"}>
          {wh.lakekeeperWarehouseId ? "Provisioned" : "Pending"}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (wh) => (
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground hover:text-destructive"
          onClick={() => onDelete(wh)}
          aria-label={`Delete ${wh.name}`}
        >
          <Trash2 className="h-4 w-4" />
          Delete
        </Button>
      ),
    },
  ];
}

/**
 * Control Plane → Warehouses
 *
 * Platform-admin list of embedded-catalog warehouses (one per workspace/domain).
 * Creating a warehouse provisions the underlying lakekeeper warehouse. Each row
 * links to a detail page for its catalog roles, grants, and Catalog Clients.
 */
export default function Warehouses() {
  const warehouses = useQuery<Warehouse[], ApiError>({
    queryKey: warehousesQueryKey,
    queryFn: listWarehouses,
    retry: false,
  });
  const storage = useQuery({ queryKey: ["control-plane", "storage"], queryFn: listStorage, retry: false });
  const workspaces = useMyWorkspaces(true);
  const queryClient = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteWarehouse,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: warehousesQueryKey }),
  });

  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Warehouse | null>(null);

  // Optional `?workspace=<id>` filter — set when arriving from a workspace's
  // Resources tab so the list is scoped to that one workspace's warehouses.
  const [searchParams] = useSearchParams();
  const wsFilter = searchParams.get("workspace");
  const wsName = (id: string) => workspaces.data?.find((w) => w.id === id)?.name ?? id.slice(0, 8);
  const rows = (warehouses.data ?? []).filter((w) => !wsFilter || w.workspaceId === wsFilter);
  const storageName = (id: string) => storage.data?.find((s) => s.id === id)?.name ?? id.slice(0, 8);

  const total = rows.length;
  const provisioned = rows.filter((w) => w.lakekeeperWarehouseId).length;
  const storageCount = storage.data?.length ?? 0;

  return (
    <div className="space-y-4" data-testid="warehouses-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <h1 className="text-2xl font-semibold leading-tight">Warehouses</h1>
          <p className="text-sm text-muted-foreground">
            Embedded-catalog warehouses. Creating one provisions the lakekeeper
            warehouse over a storage connection; open a warehouse to manage its
            roles, grants, and Catalog Clients.
          </p>
        </div>
        <Button className="shrink-0" onClick={() => setAddOpen(true)}>
          <Plus className="h-4 w-4" />
          Add warehouse
        </Button>
      </div>

      <SummaryStrip
        items={[
          { label: "warehouses", value: total },
          { label: "provisioned", value: provisioned },
          { label: "storage connections", value: storageCount },
        ]}
      />

      {wsFilter && (
        <div className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm">
          <span className="text-muted-foreground">
            Filtered to workspace <span className="font-medium text-foreground">{wsName(wsFilter)}</span>
          </span>
          <Link to="/control-plane/warehouses" className="ml-auto text-primary hover:underline">
            Show all workspaces
          </Link>
        </div>
      )}

      {warehouses.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load warehouses. Please try again.</AlertDescription>
        </Alert>
      ) : warehouses.isLoading ? (
        <ListSkeleton rows={6} cols={5} />
      ) : (
        <FilterableTable
          rows={rows}
          columns={warehouseColumns(wsName, storageName, setDeleteTarget)}
          getId={(wh) => wh.id}
          searchPlaceholder="Search warehouses…"
          initialSort={{ key: "name", dir: "asc" }}
          pageSize={25}
          bordered
          emptyText={
            wsFilter ? (
              "No warehouses for this workspace."
            ) : (
              <EmptyState
                icon={Boxes}
                title="No warehouses yet"
                description="Provision an embedded-catalog warehouse over a storage connection."
                action={
                  <Button size="sm" onClick={() => setAddOpen(true)}>
                    <Plus className="mr-1 h-4 w-4" />
                    Add warehouse
                  </Button>
                }
              />
            )
          }
        />
      )}

      <AddWarehouseDialog open={addOpen} onOpenChange={setAddOpen} />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete warehouse?"}
        description="The egret warehouse record is removed. (The underlying lakekeeper warehouse is not dropped.)"
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </div>
  );
}

function AddWarehouseDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const storage = useQuery({ queryKey: ["control-plane", "storage"], queryFn: listStorage, retry: false });
  const workspaces = useMyWorkspaces(open);
  const createMutation = useMutation<Warehouse, ApiError, CreateWarehouseInput>({
    mutationFn: createWarehouse,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: warehousesQueryKey }),
  });

  const [name, setName] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [storageId, setStorageId] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setWorkspaceId("");
    setStorageId("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createMutation.mutateAsync({
        name: name.trim(),
        workspaceId,
        storageConnectionId: storageId,
      });
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
              Provisions a lakekeeper warehouse over the selected storage connection.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpw-name">Name</Label>
              <Input
                id="cpw-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. analytics"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpw-workspace">Workspace</Label>
              <Select value={workspaceId} onValueChange={setWorkspaceId}>
                <SelectTrigger id="cpw-workspace" className="w-full">
                  <SelectValue placeholder="Select a workspace" />
                </SelectTrigger>
                <SelectContent>
                  {(workspaces.data ?? []).map((w) => (
                    <SelectItem key={w.id} value={w.id}>
                      {w.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpw-storage">Storage connection</Label>
              <Select value={storageId} onValueChange={setStorageId}>
                <SelectTrigger id="cpw-storage" className="w-full">
                  <SelectValue placeholder="Select a storage connection" />
                </SelectTrigger>
                <SelectContent>
                  {(storage.data ?? []).map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name} ({s.bucket})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
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
            <Button
              type="submit"
              disabled={createMutation.isPending || !workspaceId || !storageId}
            >
              {createMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Add warehouse
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
