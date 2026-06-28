import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, Loader2, Pencil } from "lucide-react";

import { type ApiError } from "@/api/client";
import { type Warehouse, useRenameWarehouse, useWarehouse } from "@/api/catalog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import CatalogBrowse from "./CatalogBrowse";
import CatalogAccess from "./CatalogAccess";

export default function WarehouseDetail() {
  const params = useParams();
  const ws = params.workspace!;
  const wid = params.wid!;

  const warehouse = useWarehouse(ws, wid);

  if (warehouse.isLoading) {
    return (
      <div className="p-6">
        <BackLink ws={ws} />
        <div className="mt-4">
          <ListSkeleton rows={2} cols={2} />
        </div>
      </div>
    );
  }

  if (warehouse.isError) {
    const status = (warehouse.error as ApiError | null)?.status;
    return (
      <div className="p-6">
        <BackLink ws={ws} />
        <div className="mt-4">
          {status === 403 ? (
            <Alert>
              <AlertTitle>Insufficient permissions</AlertTitle>
              <AlertDescription>
                Requires workspace-admin to manage catalog warehouses.
              </AlertDescription>
            </Alert>
          ) : status === 404 ? (
            <Alert variant="destructive">
              <AlertTitle>Warehouse not found</AlertTitle>
              <AlertDescription>
                This warehouse does not exist in this workspace.
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive">
              <AlertTitle>Failed to load warehouse</AlertTitle>
              <AlertDescription>Something went wrong. Please try again.</AlertDescription>
            </Alert>
          )}
        </div>
      </div>
    );
  }

  const wh = warehouse.data!;

  return (
    <div className="flex flex-col gap-6 p-6">
      <div>
        <BackLink ws={ws} />
        <div className="mt-2 flex items-baseline gap-3">
          <h1 className="text-xl font-semibold">{wh.name}</h1>
          {wh.storage_conn_name && (
            <span className="text-sm text-muted-foreground">
              on {wh.storage_conn_name}
            </span>
          )}
        </div>
      </div>

      <Tabs defaultValue="browse">
        <TabsList>
          <TabsTrigger value="browse">Browse</TabsTrigger>
          <TabsTrigger value="access">Access</TabsTrigger>
          <TabsTrigger value="details">Details</TabsTrigger>
        </TabsList>

        <TabsContent value="browse">
          <CatalogBrowse ws={ws} wid={wid} />
        </TabsContent>

        <TabsContent value="access">
          <div className="p-4">
            <CatalogAccess ws={ws} wid={wid} />
          </div>
        </TabsContent>

        <TabsContent value="details">
          <DetailsTab ws={ws} warehouse={wh} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function BackLink({ ws }: { ws: string }) {
  return (
    <Link
      to={`/explore/${ws}/catalog`}
      className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground hover:underline"
    >
      <ArrowLeft className="h-4 w-4" />
      All warehouses
    </Link>
  );
}

function DetailsTab({ ws, warehouse }: { ws: string; warehouse: Warehouse }) {
  const [renameOpen, setRenameOpen] = useState(false);

  return (
    <div className="p-4">
      <dl className="grid max-w-lg grid-cols-[10rem_1fr] gap-y-3 text-sm">
        <dt className="text-muted-foreground">Name</dt>
        <dd className="font-medium">{warehouse.name}</dd>

        <dt className="text-muted-foreground">Storage connection</dt>
        <dd>
          {warehouse.storage_conn_name ?? (
            <span className="text-muted-foreground/50">—</span>
          )}
        </dd>

        <dt className="text-muted-foreground">Created</dt>
        <dd>
          <RelativeTime iso={warehouse.created_at} />
        </dd>
      </dl>

      <div className="mt-6">
        <Button variant="outline" size="sm" onClick={() => setRenameOpen(true)}>
          <Pencil className="h-4 w-4" />
          Rename
        </Button>
      </div>

      <RenameDialog
        ws={ws}
        warehouse={warehouse}
        open={renameOpen}
        onOpenChange={setRenameOpen}
      />
    </div>
  );
}

function RenameDialog({
  ws,
  warehouse,
  open,
  onOpenChange,
}: {
  ws: string;
  warehouse: Warehouse;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const renameMutation = useRenameWarehouse(ws);
  const [name, setName] = useState(warehouse.name);
  const [error, setError] = useState<string | null>(null);

  const canSubmit = name.trim().length > 0 && !renameMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
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
              <Label htmlFor="wh-detail-rename">Name</Label>
              <Input
                id="wh-detail-rename"
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
