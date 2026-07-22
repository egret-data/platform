import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Boxes, Database, HardDrive, Pencil, Trash2 } from "lucide-react";

import {
  type Storage,
  deleteStorage,
  getStorage,
  listWarehouses,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { AddStorageDialog, storageQueryKey } from "@/routes/control-plane/ControlPlaneStorage";
import { StorageObjectBrowser } from "@/routes/control-plane/StorageBrowser";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["overview", "warehouses", "browse"] as const;

/**
 * Control Plane → Storage → detail.
 *
 * A per-connection page for a global storage connection: its configuration,
 * the warehouses whose data lives on it, and an object browser. Tabs live in
 * `?tab=`; editing reuses the shared AddStorageDialog.
 */
export default function StorageDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [sp, setSp] = useSearchParams();
  const tabParam = sp.get("tab") ?? "";
  const tab = (TABS as readonly string[]).includes(tabParam) ? tabParam : "overview";
  const setTab = (t: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", t);
        return n;
      },
      { replace: true },
    );

  const [editOpen, setEditOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const connQ = useQuery<Storage, ApiError>({
    queryKey: ["control-plane", "storage", id],
    queryFn: () => getStorage(id),
    retry: false,
  });
  const conn = connQ.data;

  const deleteMut = useMutation<void, ApiError, void>({
    mutationFn: () => deleteStorage(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: storageQueryKey });
      navigate("/control-plane/storage");
    },
  });

  const backLink = (
    <Link
      to="/control-plane/storage"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Storage
    </Link>
  );

  if (connQ.isError) {
    return (
      <div className="space-y-5">
        {backLink}
        <Alert variant="destructive">
          <AlertDescription>
            {connQ.error?.message ?? "Couldn't load this storage connection."}
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {backLink}

      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0 space-y-3">
          <div className="flex items-center gap-3">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <HardDrive className="h-6 w-6" />
            </div>
            <div className="min-w-0">
              <h1 className="truncate text-2xl font-semibold leading-tight">
                {conn?.name ?? "Storage"}
              </h1>
              {conn ? (
                <Badge variant="secondary" className="mt-1">
                  {conn.provider}
                </Badge>
              ) : null}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <Fact icon={Boxes} label="Bucket" value={conn?.bucket ?? "—"} mono />
            <Fact icon={HardDrive} label="Endpoint" value={conn?.endpoint ?? "AWS default"} />
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={() => setEditOpen(true)}>
            <Pencil className="mr-1 h-4 w-4" /> Edit
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="text-destructive hover:text-destructive"
            onClick={() => setDeleteOpen(true)}
          >
            <Trash2 className="mr-1 h-4 w-4" /> Delete
          </Button>
        </div>
      </div>

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="overview">
            <HardDrive className="h-4 w-4" />
            Overview
          </TabsTrigger>
          <TabsTrigger value="warehouses">
            <Boxes className="h-4 w-4" />
            Warehouses
          </TabsTrigger>
          <TabsTrigger value="browse">
            <Database className="h-4 w-4" />
            Browse
          </TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="mt-4">
          <OverviewTab conn={conn} loading={connQ.isLoading} onEdit={() => setEditOpen(true)} />
        </TabsContent>
        <TabsContent value="warehouses" className="mt-4">
          <WarehousesTab storageId={id} />
        </TabsContent>
        <TabsContent value="browse" className="mt-4">
          <StorageObjectBrowser storageId={id} />
        </TabsContent>
      </Tabs>

      <AddStorageDialog open={editOpen} onOpenChange={setEditOpen} editing={conn ?? null} />
      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={conn ? `Delete «${conn.name}»?` : "Delete storage connection?"}
        description="This storage connection will be removed. Any catalogs or workspaces that reference it may break."
        confirmText="delete"
        onConfirm={() => deleteMut.mutate()}
      />
    </div>
  );
}

function Fact({
  icon: Icon,
  label,
  value,
  mono,
}: {
  icon: typeof HardDrive;
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-center gap-1.5 text-muted-foreground">
      <Icon className="h-4 w-4" />
      <span className="text-xs uppercase tracking-wide">{label}</span>
      <span className={mono ? "font-mono text-xs text-foreground" : "text-foreground"}>{value}</span>
    </div>
  );
}

function OverviewTab({
  conn,
  loading,
  onEdit,
}: {
  conn: Storage | undefined;
  loading: boolean;
  onEdit: () => void;
}) {
  if (loading) return <ListSkeleton rows={6} cols={2} />;
  if (!conn) return null;

  const rows: { k: string; v: string; mono?: boolean }[] = [
    { k: "Provider", v: conn.provider },
    { k: "Bucket", v: conn.bucket, mono: true },
    { k: "Region", v: conn.region || "—" },
    { k: "Endpoint", v: conn.endpoint ?? "AWS default" },
    { k: "Key prefix", v: conn.key_prefix || "—", mono: true },
    { k: "Access key", v: conn.access_key, mono: true },
    { k: "Path-style URL", v: conn.path_style ? "Enabled" : "Disabled" },
    { k: "Purpose", v: conn.purpose || "—" },
  ];

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-base font-semibold">Configuration</h2>
          <p className="text-sm text-muted-foreground">
            Connection settings for this S3-compatible object store.
          </p>
        </div>
        <Button size="sm" variant="outline" className="shrink-0" onClick={onEdit}>
          <Pencil className="mr-1 h-4 w-4" /> Edit
        </Button>
      </div>
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-48">Setting</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((r) => (
              <TableRow key={r.k}>
                <TableCell className="align-middle font-medium">{r.k}</TableCell>
                <TableCell
                  className={
                    r.mono
                      ? "align-middle font-mono text-xs text-muted-foreground"
                      : "align-middle text-muted-foreground"
                  }
                >
                  {r.v}
                </TableCell>
              </TableRow>
            ))}
            <TableRow>
              <TableCell className="align-middle font-medium">Secret</TableCell>
              <TableCell className="align-middle">
                <Badge variant={conn.secret_ref ? "secondary" : "outline"}>
                  {conn.secret_ref ? "Configured" : "Not set"}
                </Badge>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </section>
  );
}

function WarehousesTab({ storageId }: { storageId: string }) {
  const whQ = useQuery({
    queryKey: ["control-plane", "warehouses"],
    queryFn: listWarehouses,
    retry: false,
  });
  const workspaces = useMyWorkspaces(true);
  const wsName = (id: string) => workspaces.data?.find((w) => w.id === id)?.name ?? id.slice(0, 8);
  const rows = (whQ.data ?? []).filter((w) => w.storageConnId === storageId);

  return (
    <section className="space-y-3">
      <div className="space-y-1">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <Boxes className="h-4 w-4 text-muted-foreground" />
          Warehouses
        </h2>
        <p className="text-sm text-muted-foreground">
          Iceberg warehouses whose data lives on this connection.
        </p>
      </div>
      {whQ.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : rows.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No warehouses use this connection yet.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Warehouse</TableHead>
                <TableHead>Workspace</TableHead>
                <TableHead className="text-right">Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((w) => (
                <TableRow key={w.id}>
                  <TableCell>
                    <div className="flex min-w-0 items-center gap-2.5">
                      <Boxes className="h-4 w-4 shrink-0 text-muted-foreground" />
                      <Link
                        to={`/control-plane/warehouses/${w.id}`}
                        className="truncate font-mono font-medium text-primary hover:underline"
                      >
                        {w.name}
                      </Link>
                    </div>
                  </TableCell>
                  <TableCell className="text-muted-foreground">{wsName(w.workspaceId)}</TableCell>
                  <TableCell className="text-right">
                    <Badge variant={w.lakekeeperWarehouseId ? "secondary" : "outline"}>
                      {w.lakekeeperWarehouseId ? "Provisioned" : "Pending"}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  );
}
