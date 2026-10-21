import { useState, type ReactNode } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Boxes, Database, HardDrive, Pencil, Trash2 } from "lucide-react";

import {
  type Storage,
  type StorageInput,
  deleteStorage,
  getStorage,
  listSecrets,
  listWarehouses,
  updateStorage,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { pickableSecrets } from "@/lib/systemSecrets";
import { secretsQueryKey, storageQueryKey } from "@/routes/control-plane/ControlPlaneStorage";
import { StorageObjectBrowser } from "@/routes/control-plane/StorageBrowser";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Input } from "@/components/ui/input";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const NO_SECRET = "__none__";

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
        <Button
          size="sm"
          variant="outline"
          className="text-destructive hover:text-destructive"
          onClick={() => setDeleteOpen(true)}
        >
          <Trash2 className="mr-1 h-4 w-4" /> Delete
        </Button>
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
          <OverviewTab conn={conn} loading={connQ.isLoading} />
        </TabsContent>
        <TabsContent value="warehouses" className="mt-4">
          <WarehousesTab storageId={id} />
        </TabsContent>
        <TabsContent value="browse" className="mt-4">
          <StorageObjectBrowser storageId={id} />
        </TabsContent>
      </Tabs>

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

function ConfigRow({
  label,
  details,
  children,
}: {
  label: string;
  details: string;
  children: ReactNode;
}) {
  return (
    <TableRow>
      <TableCell className="w-44 align-middle font-medium">{label}</TableCell>
      <TableCell className="hidden align-middle text-muted-foreground md:table-cell">
        {details}
      </TableCell>
      <TableCell className="align-middle">{children}</TableCell>
    </TableRow>
  );
}

/** Read/edit config for one connection. "Edit" flips the Value cells into inputs
 * so the table is edited in place, rather than opening a full form dialog. */
function OverviewTab({ conn, loading }: { conn: Storage | undefined; loading: boolean }) {
  const qc = useQueryClient();
  const secretsQ = useQuery({ queryKey: secretsQueryKey, queryFn: listSecrets, retry: false });

  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState({
    purpose: "",
    provider: "s3",
    endpoint: "",
    region: "",
    bucket: "",
    keyPrefix: "",
    accessKey: "",
    pathStyle: true,
    secretChoice: NO_SECRET,
  });
  const set = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, ...patch }));

  const updateMut = useMutation<Storage, ApiError, StorageInput>({
    mutationFn: (input) => updateStorage(conn!.id, input),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["control-plane", "storage", conn!.id] });
      qc.invalidateQueries({ queryKey: storageQueryKey });
      setEditing(false);
    },
    onError: (e) => setError(e.message ?? "Failed to save the connection."),
  });

  const startEdit = () => {
    if (!conn) return;
    setForm({
      purpose: conn.purpose ?? "",
      provider: conn.provider,
      endpoint: conn.endpoint ?? "",
      region: conn.region,
      bucket: conn.bucket,
      keyPrefix: conn.key_prefix ?? "",
      accessKey: conn.access_key,
      pathStyle: conn.path_style,
      secretChoice: conn.secret_ref ?? NO_SECRET,
    });
    setError(null);
    setEditing(true);
  };

  const save = () => {
    if (!conn) return;
    const input: StorageInput = {
      name: conn.name,
      purpose: form.purpose.trim() || null,
      provider: form.provider,
      endpoint: form.endpoint.trim() || null,
      region: form.region.trim(),
      bucket: form.bucket.trim(),
      key_prefix: form.keyPrefix.trim() || null,
      access_key: form.accessKey.trim(),
      path_style: form.pathStyle,
    };
    if (form.secretChoice !== NO_SECRET) input.secret_ref = form.secretChoice;
    setError(null);
    updateMut.mutate(input);
  };

  if (loading) return <ListSkeleton rows={6} cols={2} />;
  if (!conn) return null;

  const secrets = pickableSecrets(secretsQ.data ?? []);
  const ro = "text-muted-foreground";
  const roMono = "font-mono text-xs text-muted-foreground";
  // Uniform width for every edit control so the Value column reads evenly.
  const ctl = "h-8 w-72";

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-base font-semibold">Configuration</h2>
          <p className="text-sm text-muted-foreground">
            Connection settings for this S3-compatible object store.
          </p>
        </div>
        {editing ? (
          <div className="flex shrink-0 gap-2">
            <Button size="sm" variant="outline" disabled={updateMut.isPending} onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button size="sm" disabled={updateMut.isPending} onClick={save}>
              Save
            </Button>
          </div>
        ) : (
          <Button size="sm" variant="outline" className="shrink-0" onClick={startEdit}>
            <Pencil className="mr-1 h-4 w-4" /> Edit
          </Button>
        )}
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-44">Setting</TableHead>
              <TableHead className="hidden md:table-cell">Details</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <ConfigRow label="Provider" details="The object-store API this connection speaks.">
              {editing ? (
                <Select value={form.provider} onValueChange={(v) => set({ provider: v })}>
                  <SelectTrigger aria-label="Provider" className={ctl}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="s3">S3 / RustFS</SelectItem>
                    <SelectItem value="gcs" disabled>
                      Google Cloud Storage — coming soon
                    </SelectItem>
                    <SelectItem value="azure" disabled>
                      Azure Blob — coming soon
                    </SelectItem>
                  </SelectContent>
                </Select>
              ) : (
                <span className={ro}>{conn.provider}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Bucket" details="The bucket that stores this connection's objects.">
              {editing ? (
                <Input
                  aria-label="Bucket"
                  className={ctl}
                  value={form.bucket}
                  onChange={(e) => set({ bucket: e.target.value })}
                />
              ) : (
                <span className={roMono}>{conn.bucket}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Region" details="The region the bucket lives in.">
              {editing ? (
                <Input
                  aria-label="Region"
                  className={ctl}
                  value={form.region}
                  onChange={(e) => set({ region: e.target.value })}
                />
              ) : (
                <span className={ro}>{conn.region || "—"}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Endpoint" details="Custom S3 endpoint URL; blank uses AWS.">
              {editing ? (
                <Input
                  aria-label="Endpoint"
                  className={ctl}
                  placeholder="Leave blank for AWS"
                  value={form.endpoint}
                  onChange={(e) => set({ endpoint: e.target.value })}
                />
              ) : (
                <span className={ro}>{conn.endpoint ?? "AWS default"}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Key prefix" details="Optional path prefix scoping objects within the bucket.">
              {editing ? (
                <Input
                  aria-label="Key prefix"
                  className={ctl}
                  placeholder="Optional"
                  value={form.keyPrefix}
                  onChange={(e) => set({ keyPrefix: e.target.value })}
                />
              ) : (
                <span className={roMono}>{conn.key_prefix || "—"}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Access key" details="The S3 access key ID used to authenticate.">
              {editing ? (
                <Input
                  aria-label="Access key"
                  className={ctl}
                  value={form.accessKey}
                  onChange={(e) => set({ accessKey: e.target.value })}
                />
              ) : (
                <span className={roMono}>{conn.access_key}</span>
              )}
            </ConfigRow>

            <ConfigRow
              label="Path-style URL"
              details="Address the bucket as endpoint/bucket instead of a subdomain."
            >
              {editing ? (
                <Select
                  value={form.pathStyle ? "on" : "off"}
                  onValueChange={(v) => set({ pathStyle: v === "on" })}
                >
                  <SelectTrigger aria-label="Path-style URL" className={ctl}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="on">Enabled</SelectItem>
                    <SelectItem value="off">Disabled</SelectItem>
                  </SelectContent>
                </Select>
              ) : (
                <span className={ro}>{conn.path_style ? "Enabled" : "Disabled"}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Purpose" details="Free-text label for what this connection is used for.">
              {editing ? (
                <Input
                  aria-label="Purpose"
                  className={ctl}
                  placeholder="Optional"
                  value={form.purpose}
                  onChange={(e) => set({ purpose: e.target.value })}
                />
              ) : (
                <span className={ro}>{conn.purpose || "—"}</span>
              )}
            </ConfigRow>

            <ConfigRow label="Secret" details="The stored secret holding the S3 secret access key.">
              {editing ? (
                <Select value={form.secretChoice} onValueChange={(v) => set({ secretChoice: v })}>
                  <SelectTrigger aria-label="Secret" className={ctl}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NO_SECRET}>No secret</SelectItem>
                    {secrets.map((s) => (
                      <SelectItem key={s.id} value={s.id}>
                        {s.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <Badge variant={conn.secret_ref ? "secondary" : "outline"}>
                  {conn.secret_ref ? "Configured" : "Not set"}
                </Badge>
              )}
            </ConfigRow>
          </TableBody>
        </Table>
      </div>

      {editing && (
        <p className="text-xs text-muted-foreground">
          To rename the connection or mint a new secret, use Edit on the Storage list.
        </p>
      )}
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
                      {/* Still a warehouse id, deliberately — see the same
                          card on `WorkspaceDetail`. `/control-plane/warehouses/:id`
                          redirects to the matching catalog (SP3c §3). */}
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
