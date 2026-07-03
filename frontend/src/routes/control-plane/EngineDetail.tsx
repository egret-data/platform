import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, Server, Trash2 } from "lucide-react";

import {
  type AddCatalogInput,
  type CreateEngineInput,
  type Engine,
  type EngineCatalog,
  type EngineStatus,
  type Secret,
  type TrinoConfig,
  type Warehouse,
  addEngineCatalog,
  deleteEngine,
  deleteEngineCatalog,
  getEngine,
  listEngineCatalogs,
  listSecrets,
  listWarehouses,
  updateEngine,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
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

type EngineWithStatus = Omit<Engine, "status"> & { status: EngineStatus };

type BadgeVariant = "success" | "warning" | "destructive" | "secondary";

function phaseVariant(phase: string): BadgeVariant {
  switch (phase.toLowerCase()) {
    case "running":
    case "ready":
      return "success";
    case "failed":
    case "error":
      return "destructive";
    case "provisioning":
    case "pending":
    case "creating":
    case "updating":
      return "warning";
    default:
      return "secondary";
  }
}

// ── raw key→value editor helpers ─────────────────────────────────────────────

type Kv = { key: string; value: string };

function recordToKv(rec: Record<string, string> | undefined): Kv[] {
  return rec ? Object.entries(rec).map(([key, value]) => ({ key, value })) : [];
}

function kvToRecord(rows: Kv[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (key) out[key] = row.value;
  }
  return out;
}

/** A small list of {key,value} input rows with add/remove. */
function KeyValueEditor({
  rows,
  onChange,
  keyPlaceholder = "key",
  valuePlaceholder = "value",
  addLabel = "Add property",
}: {
  rows: Kv[];
  onChange: (rows: Kv[]) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
  addLabel?: string;
}) {
  return (
    <div className="space-y-2">
      {rows.map((row, i) => (
        <div key={i} className="flex items-center gap-2">
          <Input
            value={row.key}
            placeholder={keyPlaceholder}
            onChange={(e) =>
              onChange(
                rows.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)),
              )
            }
          />
          <Input
            value={row.value}
            placeholder={valuePlaceholder}
            onChange={(e) =>
              onChange(
                rows.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)),
              )
            }
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
            aria-label="Remove row"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, { key: "", value: "" }])}
      >
        <Plus className="h-4 w-4" />
        {addLabel}
      </Button>
    </div>
  );
}

/**
 * Control Plane → Engine detail.
 *
 * Live status (phase + pods, polled every 5s), editable resource + Trino config,
 * multi-catalog management, and a danger-zone delete. Config/catalog edits
 * re-provision the cluster (Trino has no hot-reload; the pods roll to reload).
 */
export default function EngineDetail() {
  const { id = "" } = useParams();
  const engine = useQuery<EngineWithStatus, ApiError>({
    queryKey: ["cp", "engine", id],
    queryFn: () => getEngine(id),
    refetchInterval: 5000,
    retry: false,
  });

  const data = engine.data;

  return (
    <div className="space-y-5">
      <Link
        to="/control-plane/engines"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Engines
      </Link>

      <Card className="overflow-hidden">
        <div className="flex items-center gap-3 border-b bg-muted/30 p-5">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <Server className="h-6 w-6" />
          </div>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-2xl font-semibold leading-tight">
              {data?.name ?? "Engine"}
            </h1>
            <p className="font-mono text-xs text-muted-foreground">
              {data ? `${data.namespace}` : ""}
            </p>
          </div>
          {data && (
            <div className="flex shrink-0 items-center gap-2">
              <Badge variant="secondary">{data.type}</Badge>
              <Badge variant={phaseVariant(data.status.phase)}>
                {data.status.phase}
              </Badge>
            </div>
          )}
        </div>
      </Card>

      {engine.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {engine.error?.message ?? "Failed to load engine."}
          </AlertDescription>
        </Alert>
      ) : !data ? (
        <p className="py-14 text-center text-sm text-muted-foreground">Loading…</p>
      ) : (
        <>
          <StatusCard status={data.status} />
          <ConfigCard engine={data} />
          <CatalogsCard engine={data} />
          <DangerCard engine={data} />
        </>
      )}
    </div>
  );
}

function StatusCard({ status }: { status: EngineStatus }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Status</CardTitle>
        <CardDescription>
          {status.message ?? `Phase: ${status.phase}`}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Pod</TableHead>
              <TableHead>Phase</TableHead>
              <TableHead>Ready</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {status.pods.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell
                  colSpan={3}
                  className="py-10 text-center text-muted-foreground"
                >
                  No pods yet.
                </TableCell>
              </TableRow>
            ) : (
              status.pods.map((pod) => (
                <TableRow key={pod.name}>
                  <TableCell className="font-mono text-xs">{pod.name}</TableCell>
                  <TableCell>
                    <Badge variant={phaseVariant(pod.phase)}>{pod.phase}</Badge>
                  </TableCell>
                  <TableCell>
                    <Badge variant={pod.ready ? "success" : "secondary"}>
                      {pod.ready ? "Ready" : "Not ready"}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}

function ConfigCard({ engine }: { engine: EngineWithStatus }) {
  const queryClient = useQueryClient();
  const [coordinatorReplicas, setCoordinatorReplicas] = useState(
    engine.spec.coordinatorReplicas,
  );
  const [workerReplicas, setWorkerReplicas] = useState(engine.spec.workerReplicas);
  const [cpuMin, setCpuMin] = useState(engine.spec.cpuMin);
  const [cpuMax, setCpuMax] = useState(engine.spec.cpuMax);
  const [memory, setMemory] = useState(engine.spec.memory);

  // Curated Trino config (read from `spec.trinoConfig` if the DTO exposes it).
  const tc = engine.spec.trinoConfig;
  const [jvmHeap, setJvmHeap] = useState(tc?.jvmHeap ?? "");
  const [queryMaxMemory, setQueryMaxMemory] = useState(tc?.queryMaxMemory ?? "");
  const [queryMaxMemoryPerNode, setQueryMaxMemoryPerNode] = useState(
    tc?.queryMaxMemoryPerNode ?? "",
  );
  const [configProps, setConfigProps] = useState<Kv[]>(
    recordToKv(tc?.configProperties),
  );
  const [env, setEnv] = useState<Kv[]>(recordToKv(tc?.env));

  const [error, setError] = useState<string | null>(null);

  // Keep inputs in sync when the polled engine changes (e.g. an external update).
  useEffect(() => {
    setCoordinatorReplicas(engine.spec.coordinatorReplicas);
    setWorkerReplicas(engine.spec.workerReplicas);
    setCpuMin(engine.spec.cpuMin);
    setCpuMax(engine.spec.cpuMax);
    setMemory(engine.spec.memory);
    const next = engine.spec.trinoConfig;
    setJvmHeap(next?.jvmHeap ?? "");
    setQueryMaxMemory(next?.queryMaxMemory ?? "");
    setQueryMaxMemoryPerNode(next?.queryMaxMemoryPerNode ?? "");
    setConfigProps(recordToKv(next?.configProperties));
    setEnv(recordToKv(next?.env));
  }, [engine.spec]);

  const saveMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] }),
  });

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const trinoConfig: TrinoConfig = {};
    if (jvmHeap.trim()) trinoConfig.jvmHeap = jvmHeap.trim();
    if (queryMaxMemory.trim()) trinoConfig.queryMaxMemory = queryMaxMemory.trim();
    if (queryMaxMemoryPerNode.trim())
      trinoConfig.queryMaxMemoryPerNode = queryMaxMemoryPerNode.trim();
    trinoConfig.configProperties = kvToRecord(configProps);
    trinoConfig.env = kvToRecord(env);

    const input: CreateEngineInput = {
      name: engine.name,
      warehouseId: engine.warehouseId,
      coordinatorReplicas,
      workerReplicas,
      cpuMin: cpuMin.trim(),
      cpuMax: cpuMax.trim(),
      memory: memory.trim(),
      trinoConfig,
    };
    try {
      await saveMutation.mutateAsync(input);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to update engine.");
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Configuration</CardTitle>
        <CardDescription>
          Adjust cluster size, resources, and Trino config. Saving re-provisions
          the engine.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label htmlFor="ced-coordinators">Coordinator replicas</Label>
              <Input
                id="ced-coordinators"
                type="number"
                min={1}
                value={coordinatorReplicas}
                onChange={(e) => setCoordinatorReplicas(Number(e.target.value))}
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ced-workers">Worker replicas</Label>
              <Input
                id="ced-workers"
                type="number"
                min={0}
                value={workerReplicas}
                onChange={(e) => setWorkerReplicas(Number(e.target.value))}
                required
              />
            </div>
          </div>

          <div className="grid grid-cols-3 gap-4">
            <div className="space-y-1.5">
              <Label htmlFor="ced-cpu-min">CPU min</Label>
              <Input
                id="ced-cpu-min"
                value={cpuMin}
                onChange={(e) => setCpuMin(e.target.value)}
                placeholder="1"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ced-cpu-max">CPU max</Label>
              <Input
                id="ced-cpu-max"
                value={cpuMax}
                onChange={(e) => setCpuMax(e.target.value)}
                placeholder="2"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ced-memory">Memory</Label>
              <Input
                id="ced-memory"
                value={memory}
                onChange={(e) => setMemory(e.target.value)}
                placeholder="4Gi"
                required
              />
            </div>
          </div>

          <div className="space-y-4 rounded-lg border p-4">
            <p className="text-sm font-medium">Trino config</p>
            <div className="grid grid-cols-3 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="ced-jvm-heap">JVM heap</Label>
                <Input
                  id="ced-jvm-heap"
                  value={jvmHeap}
                  onChange={(e) => setJvmHeap(e.target.value)}
                  placeholder="2G"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="ced-qmm">Query max memory</Label>
                <Input
                  id="ced-qmm"
                  value={queryMaxMemory}
                  onChange={(e) => setQueryMaxMemory(e.target.value)}
                  placeholder="4GB"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="ced-qmmpn">Query max memory / node</Label>
                <Input
                  id="ced-qmmpn"
                  value={queryMaxMemoryPerNode}
                  onChange={(e) => setQueryMaxMemoryPerNode(e.target.value)}
                  placeholder="1GB"
                />
              </div>
            </div>

            <div className="space-y-1.5">
              <Label>Config properties</Label>
              <p className="text-xs text-muted-foreground">
                Raw <code>config.properties</code> key/value pairs.
              </p>
              <KeyValueEditor
                rows={configProps}
                onChange={setConfigProps}
                keyPlaceholder="query.max-run-time"
                valuePlaceholder="1h"
              />
            </div>

            <div className="space-y-1.5">
              <Label>Environment variables</Label>
              <p className="text-xs text-muted-foreground">
                Extra env applied to both coordinator and worker pods.
              </p>
              <KeyValueEditor
                rows={env}
                onChange={setEnv}
                keyPlaceholder="MY_VAR"
                valuePlaceholder="value"
                addLabel="Add variable"
              />
            </div>
          </div>

          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {saveMutation.isSuccess && !error && (
            <p className="rounded-md border border-primary/30 bg-primary/5 px-3 py-2 text-sm text-muted-foreground">
              Saved. The engine will restart to apply the new configuration.
            </p>
          )}

          <div className="flex justify-end">
            <Button type="submit" disabled={saveMutation.isPending}>
              {saveMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save changes
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}

// ── Catalogs ─────────────────────────────────────────────────────────────────

function CatalogsCard({ engine }: { engine: EngineWithStatus }) {
  const queryClient = useQueryClient();
  const catalogs = useQuery<EngineCatalog[], ApiError>({
    queryKey: ["cp", "engine-catalogs", engine.id],
    queryFn: () => listEngineCatalogs(engine.id),
    retry: false,
  });

  const [addOpen, setAddOpen] = useState(false);
  const [removeTarget, setRemoveTarget] = useState<EngineCatalog | null>(null);

  const invalidate = () =>
    queryClient.invalidateQueries({
      queryKey: ["cp", "engine-catalogs", engine.id],
    });

  const removeMutation = useMutation<void, ApiError, string>({
    mutationFn: (cid) => deleteEngineCatalog(engine.id, cid),
    onSuccess: () => {
      invalidate();
      setRemoveTarget(null);
    },
  });

  const rows = catalogs.data ?? [];

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="text-lg">Catalogs</CardTitle>
            <CardDescription>
              Data sources this engine can query. Adding or removing a catalog
              rolls the pods to reload.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add catalog
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {catalogs.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {catalogs.error?.message ?? "Failed to load catalogs."}
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={3}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {catalogs.isLoading ? "Loading…" : "No catalogs yet."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((cat) => (
                  <TableRow key={cat.id}>
                    <TableCell className="font-medium">
                      {cat.name}
                      {cat.isPrimary && (
                        <Badge variant="secondary" className="ml-2">
                          primary
                        </Badge>
                      )}
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{cat.kind}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {!cat.isPrimary && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-muted-foreground"
                          onClick={() => setRemoveTarget(cat)}
                        >
                          <Trash2 className="h-4 w-4" />
                          Remove
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

      <AddCatalogDialog
        engineId={engine.id}
        open={addOpen}
        onOpenChange={setAddOpen}
        onAdded={invalidate}
      />

      <ConfirmDeleteDialog
        open={removeTarget !== null}
        onOpenChange={(next) => {
          if (!next) setRemoveTarget(null);
        }}
        title={`Remove «${removeTarget?.name ?? ""}»?`}
        description="This catalog will be removed and the engine will roll to reload. This cannot be undone."
        confirmText="remove"
        onConfirm={() => {
          if (removeTarget) removeMutation.mutate(removeTarget.id);
        }}
      />
    </Card>
  );
}

const CATALOG_KINDS: { value: string; label: string }[] = [
  { value: "egret-warehouse", label: "Egret warehouse" },
  { value: "postgresql", label: "PostgreSQL" },
  { value: "tpch", label: "TPC-H" },
  { value: "tpcds", label: "TPC-DS" },
  { value: "generic", label: "Generic connector" },
];

function AddCatalogDialog({
  engineId,
  open,
  onOpenChange,
  onAdded,
}: {
  engineId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded: () => void;
}) {
  const warehouses = useQuery<Warehouse[], ApiError>({
    queryKey: ["cp", "warehouses"],
    queryFn: listWarehouses,
    retry: false,
    enabled: open,
  });
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
    enabled: open,
  });

  const addMutation = useMutation<EngineCatalog, ApiError, AddCatalogInput>({
    mutationFn: (input) => addEngineCatalog(engineId, input),
    onSuccess: () => onAdded(),
  });

  const [name, setName] = useState("");
  const [kind, setKind] = useState("egret-warehouse");
  const [error, setError] = useState<string | null>(null);

  // egret-warehouse
  const [warehouseId, setWarehouseId] = useState("");
  // postgresql
  const [host, setHost] = useState("");
  const [port, setPort] = useState("5432");
  const [database, setDatabase] = useState("");
  const [user, setUser] = useState("");
  const [secretRef, setSecretRef] = useState("");
  // generic
  const [connectorName, setConnectorName] = useState("");
  const [genericProps, setGenericProps] = useState<Kv[]>([]);

  useEffect(() => {
    if (open) reset();
  }, [open]);

  function reset() {
    setName("");
    setKind("egret-warehouse");
    setError(null);
    setWarehouseId("");
    setHost("");
    setPort("5432");
    setDatabase("");
    setUser("");
    setSecretRef("");
    setConnectorName("");
    setGenericProps([]);
    addMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  // Only provisioned warehouses (embedded catalog id resolved) can back a catalog.
  const provisioned = (warehouses.data ?? []).filter(
    (w) => w.lakekeeperWarehouseId,
  );

  function isValid(): boolean {
    if (!name.trim()) return false;
    switch (kind) {
      case "egret-warehouse":
        return warehouseId !== "";
      case "postgresql":
        return (
          host.trim() !== "" &&
          database.trim() !== "" &&
          user.trim() !== "" &&
          secretRef !== ""
        );
      case "generic":
        return connectorName.trim() !== "";
      default:
        return true; // tpch / tpcds only need a name
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    let config: Record<string, unknown> = {};
    let ref: string | undefined;
    switch (kind) {
      case "egret-warehouse":
        config = { warehouseId };
        break;
      case "postgresql":
        config = {
          host: host.trim(),
          port: Number(port),
          database: database.trim(),
          user: user.trim(),
        };
        ref = secretRef || undefined;
        break;
      case "generic":
        config = {
          connectorName: connectorName.trim(),
          properties: kvToRecord(genericProps),
        };
        break;
      default:
        config = {};
    }

    const input: AddCatalogInput = {
      name: name.trim(),
      kind,
      config,
      secretRef: ref ?? null,
    };

    try {
      await addMutation.mutateAsync(input);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to add catalog.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add catalog</DialogTitle>
            <DialogDescription>
              Attach a data source to this engine. The engine rolls to load it.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cat-name">Name</Label>
              <Input
                id="cat-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. warehouse2"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cat-kind">Kind</Label>
              <Select value={kind} onValueChange={setKind}>
                <SelectTrigger id="cat-kind" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {CATALOG_KINDS.map((k) => (
                    <SelectItem key={k.value} value={k.value}>
                      {k.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {kind === "egret-warehouse" && (
              <div className="space-y-1.5">
                <Label htmlFor="cat-warehouse">Warehouse</Label>
                <Select value={warehouseId} onValueChange={setWarehouseId}>
                  <SelectTrigger id="cat-warehouse" className="w-full">
                    <SelectValue placeholder="Select a warehouse" />
                  </SelectTrigger>
                  <SelectContent>
                    {provisioned.length === 0 ? (
                      <SelectItem value="__none__" disabled>
                        No provisioned warehouses
                      </SelectItem>
                    ) : (
                      provisioned.map((w) => (
                        <SelectItem key={w.id} value={w.id}>
                          {w.name}
                        </SelectItem>
                      ))
                    )}
                  </SelectContent>
                </Select>
              </div>
            )}

            {kind === "postgresql" && (
              <>
                <div className="grid grid-cols-3 gap-4">
                  <div className="col-span-2 space-y-1.5">
                    <Label htmlFor="cat-host">Host</Label>
                    <Input
                      id="cat-host"
                      value={host}
                      onChange={(e) => setHost(e.target.value)}
                      placeholder="pg.internal"
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cat-port">Port</Label>
                    <Input
                      id="cat-port"
                      type="number"
                      value={port}
                      onChange={(e) => setPort(e.target.value)}
                      placeholder="5432"
                      required
                    />
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-1.5">
                    <Label htmlFor="cat-database">Database</Label>
                    <Input
                      id="cat-database"
                      value={database}
                      onChange={(e) => setDatabase(e.target.value)}
                      placeholder="analytics"
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cat-user">User</Label>
                    <Input
                      id="cat-user"
                      value={user}
                      onChange={(e) => setUser(e.target.value)}
                      placeholder="trino"
                      required
                    />
                  </div>
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cat-secret">Password secret</Label>
                  <Select value={secretRef} onValueChange={setSecretRef}>
                    <SelectTrigger id="cat-secret" className="w-full">
                      <SelectValue placeholder="Select a secret" />
                    </SelectTrigger>
                    <SelectContent>
                      {(secrets.data ?? []).length === 0 ? (
                        <SelectItem value="__none__" disabled>
                          No secrets
                        </SelectItem>
                      ) : (
                        (secrets.data ?? []).map((s) => (
                          <SelectItem key={s.id} value={s.id}>
                            {s.name}
                          </SelectItem>
                        ))
                      )}
                    </SelectContent>
                  </Select>
                </div>
              </>
            )}

            {kind === "generic" && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="cat-connector">Connector name</Label>
                  <Input
                    id="cat-connector"
                    value={connectorName}
                    onChange={(e) => setConnectorName(e.target.value)}
                    placeholder="e.g. mysql"
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>Properties</Label>
                  <KeyValueEditor
                    rows={genericProps}
                    onChange={setGenericProps}
                    keyPlaceholder="connection-url"
                    valuePlaceholder="jdbc:mysql://…"
                  />
                </div>
              </>
            )}

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={addMutation.isPending || !isValid()}>
              {addMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Add catalog
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DangerCard({ engine }: { engine: EngineWithStatus }) {
  const navigate = useNavigate();
  const [deleteOpen, setDeleteOpen] = useState(false);
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteEngine,
    onSuccess: () => navigate("/control-plane/engines"),
  });

  return (
    <Card className="border-destructive/40">
      <CardHeader>
        <CardTitle className="text-lg">Danger zone</CardTitle>
        <CardDescription>
          Deleting this engine tears down its Kubernetes resources. This cannot be
          undone.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Button
          variant="destructive"
          onClick={() => setDeleteOpen(true)}
          disabled={deleteMutation.isPending}
        >
          {deleteMutation.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Trash2 className="h-4 w-4" />
          )}
          Delete engine
        </Button>
      </CardContent>

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete «${engine.name}»?`}
        description="This engine and its Kubernetes resources (cluster, catalog, secret) will be removed."
        confirmText="delete"
        onConfirm={() => deleteMutation.mutate(engine.id)}
      />
    </Card>
  );
}
