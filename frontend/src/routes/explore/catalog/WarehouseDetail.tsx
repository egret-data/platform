import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, Loader2, Pencil } from "lucide-react";

import { type ApiError } from "@/api/client";
import {
  type Warehouse,
  type WarehouseTask,
  useRenameWarehouse,
  useWarehouse,
  useWarehouseStatistics,
  useWarehouseTasks,
} from "@/api/catalog";
import { Badge } from "@/components/ui/badge";
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
          <TabsTrigger value="statistics">Statistics</TabsTrigger>
          <TabsTrigger value="tasks">Tasks</TabsTrigger>
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

        <TabsContent value="statistics">
          <StatisticsTab ws={ws} wid={wid} />
        </TabsContent>

        <TabsContent value="tasks">
          <TasksTab ws={ws} wid={wid} />
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

function StatisticsTab({ ws, wid }: { ws: string; wid: string }) {
  const stats = useWarehouseStatistics(ws, wid);

  if (stats.isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton rows={2} cols={2} />
      </div>
    );
  }

  const entries = stats.data?.entries ?? [];
  if (entries.length === 0) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        No statistics yet — lakekeeper computes these hourly.
      </div>
    );
  }

  const latest = entries[0];
  const history = entries.slice(1);

  return (
    <div className="flex flex-col gap-6 p-4">
      <div className="flex flex-wrap items-baseline gap-x-8 gap-y-2">
        <Stat label="Tables" value={latest.number_of_tables} />
        <Stat label="Views" value={latest.number_of_views} />
        <span className="text-sm text-muted-foreground">
          as of <RelativeTime iso={latest.timestamp} />
        </span>
      </div>

      {history.length > 0 && (
        <table className="max-w-xl text-sm">
          <thead>
            <tr className="border-b text-left text-muted-foreground">
              <th className="py-1.5 pr-6 font-medium">Time</th>
              <th className="py-1.5 pr-6 font-medium">Tables</th>
              <th className="py-1.5 font-medium">Views</th>
            </tr>
          </thead>
          <tbody>
            {history.map((e) => (
              <tr key={e.timestamp} className="border-b last:border-0">
                <td className="py-1.5 pr-6">
                  <RelativeTime iso={e.timestamp} />
                </td>
                <td className="py-1.5 pr-6">{e.number_of_tables}</td>
                <td className="py-1.5">{e.number_of_views}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex flex-col">
      <span className="text-2xl font-semibold tabular-nums">{value}</span>
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
}

function TasksTab({ ws, wid }: { ws: string; wid: string }) {
  const tasks = useWarehouseTasks(ws, wid);

  if (tasks.isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton rows={3} cols={4} />
      </div>
    );
  }

  const rows = tasks.data ?? [];
  if (rows.length === 0) {
    return <div className="p-4 text-sm text-muted-foreground">No tasks.</div>;
  }

  return (
    <div className="p-4">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b text-left text-muted-foreground">
            <th className="py-1.5 pr-6 font-medium">Type</th>
            <th className="py-1.5 pr-6 font-medium">Status</th>
            <th className="py-1.5 pr-6 font-medium">Entity</th>
            <th className="py-1.5 pr-6 font-medium">Created</th>
            <th className="py-1.5 font-medium">Attempt</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((t) => (
            <tr key={t.task_id} className="border-b last:border-0">
              <td className="py-1.5 pr-6 font-mono text-xs">{t.task_type}</td>
              <td className="py-1.5 pr-6">
                <Badge variant={statusVariant(t.status)}>{t.status}</Badge>
              </td>
              <td className="py-1.5 pr-6">
                {t.entity_name ?? <span className="text-muted-foreground/50">—</span>}
              </td>
              <td className="py-1.5 pr-6">
                <RelativeTime iso={t.created_at} />
              </td>
              <td className="py-1.5">{t.attempt}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function statusVariant(
  status: WarehouseTask["status"],
): "default" | "secondary" | "destructive" | "outline" {
  switch (status) {
    case "RUNNING":
      return "default";
    case "SUCCESS":
      return "secondary";
    case "FAILED":
      return "destructive";
    default: // SCHEDULED / STOPPING / CANCELLED
      return "outline";
  }
}
