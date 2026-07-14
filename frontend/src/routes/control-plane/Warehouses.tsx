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
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const COLUMNS = ["Name", "Workspace", "Storage", "Provisioned", ""];

export const warehousesQueryKey = ["control-plane", "warehouses"] as const;

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

  return (
    <Card data-testid="warehouses-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Boxes className="h-5 w-5 text-muted-foreground" />
              Warehouses
            </CardTitle>
            <CardDescription>
              Embedded-catalog warehouses. Creating one provisions the lakekeeper
              warehouse over a storage connection; open a warehouse to manage its
              roles, grants, and Catalog Clients.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add warehouse
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {(() => {
          const all = rows;
          const total = all.length;
          const provisioned = all.filter((w) => w.lakekeeperWarehouseId).length;
          const storageCount = storage.data?.length ?? 0;
          return (
            <SummaryStrip
              items={[
                { label: "warehouses", value: total },
                { label: "provisioned", value: provisioned },
                { label: "storage connections", value: storageCount },
              ]}
            />
          );
        })()}
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
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                {COLUMNS.map((col) => (
                  <TableHead key={col || "actions"} className={col === "" ? "text-right" : undefined}>
                    {col || <span className="sr-only">Actions</span>}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell colSpan={COLUMNS.length} className="py-10 text-center text-muted-foreground">
                    {warehouses.isLoading
                      ? "Loading…"
                      : wsFilter
                        ? "No warehouses for this workspace."
                        : "No warehouses yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((wh) => (
                  <TableRow key={wh.id}>
                    <TableCell className="font-medium">
                      <Link to={`/control-plane/warehouses/${wh.id}`} className="text-primary font-medium hover:underline">
                        {wh.name}
                      </Link>
                    </TableCell>
                    <TableCell className="text-muted-foreground">{wsName(wh.workspaceId)}</TableCell>
                    <TableCell className="text-muted-foreground">{storageName(wh.storageConnId)}</TableCell>
                    <TableCell>
                      <Badge variant={wh.lakekeeperWarehouseId ? "secondary" : "outline"}>
                        {wh.lakekeeperWarehouseId ? "Provisioned" : "Pending"}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right">
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
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

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
    </Card>
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
