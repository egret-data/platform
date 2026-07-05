import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, Server, Trash2 } from "lucide-react";

import {
  type AddCatalogInput,
  type Engine,
  type EngineCatalog,
  type EngineStatus,
  type Secret,
  type Warehouse,
  addEngineCatalog,
  deleteEngine,
  deleteEngineCatalog,
  getEngine,
  listEngineCatalogs,
  listSecrets,
  listWarehouses,
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { type Kv, kvToRecord } from "./EngineResourceGroups";
import {
  useEngineConfigForm,
  type EngineConfigForm,
} from "./useEngineConfigForm";
import { connectorByName, connectors } from "./config/connectorCatalog";
import { KeyedPropertyList } from "./config/KeyedPropertyList";
import { ImageSection } from "./config/ImageSection";
import { ServerSection } from "./config/ServerSection";
import { CoordinatorSection } from "./config/CoordinatorSection";
import { WorkerSection } from "./config/WorkerSection";
import { WorkloadSection } from "./config/WorkloadSection";
import { PodSection } from "./config/PodSection";
import { ContainersMountsSection } from "./config/ContainersMountsSection";
import { ValidityProvider } from "./config/validityRegistry";
import { ReviewChangesDialog } from "./config/ReviewChangesDialog";
import { EngineAutoscaling } from "./EngineAutoscaling";
import { EngineSecurity } from "./EngineSecurity";
import EngineMonitor from "./EngineMonitor";

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
        <p className="py-14 text-center text-sm text-muted-foreground">
          Loading…
        </p>
      ) : (
        <EngineTabs engine={data} engineId={id} />
      )}
    </div>
  );
}

/**
 * Top-level engine tabs: Overview / Configuration / Security & Exposure /
 * Monitor. The config form hook is instantiated here so the Configuration and
 * Security tabs share a single form state (and a single Save action).
 */
function EngineTabs({
  engine,
  engineId,
}: {
  engine: EngineWithStatus;
  engineId: string;
}) {
  const cfg = useEngineConfigForm(engine);

  return (
    <Tabs defaultValue="overview" className="space-y-5">
      <TabsList>
        <TabsTrigger value="overview">Overview</TabsTrigger>
        <TabsTrigger value="catalogs">Catalogs</TabsTrigger>
        <TabsTrigger value="configuration">Configuration</TabsTrigger>
        <TabsTrigger value="security">Security &amp; Exposure</TabsTrigger>
        <TabsTrigger value="monitor">Monitor</TabsTrigger>
      </TabsList>

      <TabsContent value="overview" className="space-y-5">
        <StatusCard status={engine.status} />
        <DangerCard engine={engine} />
      </TabsContent>

      <TabsContent value="catalogs">
        <CatalogsCard engine={engine} />
      </TabsContent>

      <TabsContent value="configuration">
        <ConfigurationTab cfg={cfg} imageVersion={engine.spec.imageVersion} />
      </TabsContent>

      <TabsContent value="security">
        <SecurityTab cfg={cfg} />
      </TabsContent>

      <TabsContent value="monitor">
        <EngineMonitor engineId={engineId} />
      </TabsContent>
    </Tabs>
  );
}

/**
 * Shared "Save changes" footer for the Configuration and Security tabs. Both
 * act on the same `useEngineConfigForm` instance passed down as `cfg`.
 */
function SaveBar({
  cfg,
  disabled = false,
  review = false,
}: {
  cfg: EngineConfigForm;
  /** Extra disable condition (e.g. an invalid raw-JSON override) on top of `!dirty || saving`. */
  disabled?: boolean;
  /** Show the Review-changes button (diffs built vs stored trinoConfig). */
  review?: boolean;
}) {
  const { error, saved, dirty, saving, save } = cfg;
  const [reviewOpen, setReviewOpen] = useState(false);
  return (
    <div className="space-y-4">
      {error && <p className="text-sm text-destructive">{error}</p>}

      {saved && !error && (
        <p className="rounded-md border border-primary/30 bg-primary/5 px-3 py-2 text-sm text-muted-foreground">
          Saved. The engine will restart to apply the new configuration.
        </p>
      )}

      <div className="flex justify-end gap-2">
        {review && (
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => setReviewOpen(true)}
              disabled={!dirty}
            >
              Review changes
            </Button>
            <ReviewChangesDialog
              before={cfg.storedTrinoConfig ?? {}}
              after={cfg.builtTrinoConfig}
              open={reviewOpen}
              onOpenChange={setReviewOpen}
            />
          </>
        )}
        <Button
          type="button"
          onClick={save}
          disabled={!dirty || saving || disabled}
        >
          {saving && <Loader2 className="h-4 w-4 animate-spin" />}
          Save changes
        </Button>
      </div>
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
                  <TableCell className="font-mono text-xs">
                    {pod.name}
                  </TableCell>
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

// Per-role config form model + serialization lives in `useEngineConfigForm.ts`;
// the per-role editor UI lives in `EngineRoleConfig.tsx`.

// Configuration left-nav sections — a flat 7-item list mirroring the Trino
// chart's top-level blocks. Each key matches a branch in the content pane
// below.
const NAV_ITEMS: { key: string; label: string }[] = [
  { key: "image", label: "Image" },
  { key: "server", label: "Server" },
  { key: "coordinator", label: "Coordinator" },
  { key: "worker", label: "Worker" },
  { key: "workload", label: "Workload" },
  { key: "pod", label: "Pod & Containers" },
  { key: "autoscaling", label: "Autoscaling" },
];

function ConfigurationTab({
  cfg,
  imageVersion,
}: {
  cfg: EngineConfigForm;
  imageVersion: string;
}) {
  const {
    autoscaleMode,
    setAutoscaleMode,
    autoscaleHpa,
    setAutoscaleHpa,
    autoscaleKeda,
    setAutoscaleKeda,
    sectionCounts,
  } = cfg;

  // Goes true when any raw-JSON override (any section) fails to parse; blocks
  // Save. Every `RawJsonField` under the single `ValidityProvider` below reports
  // into this one registry — no per-section validity gates.
  const [rawInvalid, setRawInvalid] = useState(false);

  // Active left-nav section.
  const [section, setSection] = useState("image");

  // Non-default field count per nav item (drives the badges).
  const countFor = (key: string): number => sectionCounts[key] ?? 0;

  return (
    <Card>
      <CardHeader>
        <div className="space-y-1.5">
          <CardTitle className="text-lg">Configuration</CardTitle>
          <CardDescription>
            Cluster-wide, coordinator, and worker Trino config. Saving
            re-provisions the engine.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <div className="flex flex-col gap-4 md:flex-row">
            <nav className="shrink-0 space-y-1 md:w-48">
              {NAV_ITEMS.map((item) => {
                const count = countFor(item.key);
                const active = section === item.key;
                return (
                  <button
                    key={item.key}
                    type="button"
                    onClick={() => setSection(item.key)}
                    className={`flex w-full items-center justify-between rounded-md px-3 py-2 text-left text-sm transition-colors hover:bg-muted ${
                      active ? "bg-muted font-medium" : "text-muted-foreground"
                    }`}
                  >
                    <span>{item.label}</span>
                    {count > 0 && (
                      <Badge variant="secondary" className="ml-2">
                        {count}
                      </Badge>
                    )}
                  </button>
                );
              })}
            </nav>

            <ValidityProvider onAnyInvalidChange={setRawInvalid}>
              <div className="min-w-0 flex-1">
                {section === "image" && (
                  <ImageSection
                    cfg={cfg}
                    defaultImage={`trinodb/trino:${imageVersion}`}
                  />
                )}
                {section === "server" && <ServerSection cfg={cfg} />}
                {section === "coordinator" && <CoordinatorSection cfg={cfg} />}
                {section === "worker" && <WorkerSection cfg={cfg} />}
                {section === "workload" && <WorkloadSection cfg={cfg} />}
                {section === "pod" && (
                  <div className="space-y-5">
                    <PodSection
                      serviceAccount={cfg.serviceAccount}
                      setServiceAccount={cfg.setServiceAccount}
                      podSecurityContext={cfg.podSecurityContext}
                      setPodSecurityContext={cfg.setPodSecurityContext}
                      containerSecurityContext={cfg.containerSecurityContext}
                      setContainerSecurityContext={
                        cfg.setContainerSecurityContext
                      }
                      deploymentStrategy={cfg.deploymentStrategy}
                      setDeploymentStrategy={cfg.setDeploymentStrategy}
                      deploymentAnnotations={cfg.deploymentAnnotations}
                      setDeploymentAnnotations={cfg.setDeploymentAnnotations}
                      revisionHistoryLimit={cfg.revisionHistoryLimit}
                      setRevisionHistoryLimit={cfg.setRevisionHistoryLimit}
                      progressDeadlineSeconds={cfg.progressDeadlineSeconds}
                      setProgressDeadlineSeconds={
                        cfg.setProgressDeadlineSeconds
                      }
                      shareProcessNamespace={cfg.shareProcessNamespace}
                      setShareProcessNamespace={cfg.setShareProcessNamespace}
                      env={cfg.env}
                      setEnv={cfg.setEnv}
                    />
                    <ContainersMountsSection
                      initContainers={cfg.initContainers}
                      setInitContainers={cfg.setInitContainers}
                      sidecarContainers={cfg.sidecarContainers}
                      setSidecarContainers={cfg.setSidecarContainers}
                      envFrom={cfg.envFrom}
                      setEnvFrom={cfg.setEnvFrom}
                      configMounts={cfg.configMounts}
                      setConfigMounts={cfg.setConfigMounts}
                      secretMounts={cfg.secretMounts}
                      setSecretMounts={cfg.setSecretMounts}
                    />
                  </div>
                )}
                {section === "autoscaling" && (
                  <EngineAutoscaling
                    mode={autoscaleMode}
                    setMode={setAutoscaleMode}
                    hpa={autoscaleHpa}
                    setHpa={setAutoscaleHpa}
                    keda={autoscaleKeda}
                    setKeda={setAutoscaleKeda}
                  />
                )}
              </div>
            </ValidityProvider>
          </div>

          <SaveBar cfg={cfg} disabled={rawInvalid} review />
        </div>
      </CardContent>
    </Card>
  );
}

function SecurityTab({ cfg }: { cfg: EngineConfigForm }) {
  const {
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
    jwksUri,
    setJwksUri,
    requiredAudience,
    setRequiredAudience,
    ingressEnabled,
    setIngressEnabled,
    ingressHost,
    setIngressHost,
    ingressClassName,
    setIngressClassName,
    ingressTlsSecret,
    setIngressTlsSecret,
    ingressAnnotations,
    setIngressAnnotations,
  } = cfg;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Security &amp; Exposure</CardTitle>
        <CardDescription>
          Client authentication and coordinator exposure. Saving re-provisions
          the engine.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <EngineSecurity
            passwordUsers={passwordUsers}
            setPasswordUsers={setPasswordUsers}
            jwtEnabled={jwtEnabled}
            setJwtEnabled={setJwtEnabled}
            jwksUri={jwksUri}
            setJwksUri={setJwksUri}
            requiredAudience={requiredAudience}
            setRequiredAudience={setRequiredAudience}
            ingressEnabled={ingressEnabled}
            setIngressEnabled={setIngressEnabled}
            ingressHost={ingressHost}
            setIngressHost={setIngressHost}
            ingressClassName={ingressClassName}
            setIngressClassName={setIngressClassName}
            ingressTlsSecret={ingressTlsSecret}
            setIngressTlsSecret={setIngressTlsSecret}
            ingressAnnotations={ingressAnnotations}
            setIngressAnnotations={setIngressAnnotations}
          />

          <SaveBar cfg={cfg} />
        </div>
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
  // True once the user explicitly picks "Other (custom)…", so the empty initial
  // state shows the placeholder rather than pre-selecting the custom option.
  const [customConnector, setCustomConnector] = useState(false);
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
    setCustomConnector(false);
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
                  <Label htmlFor="cat-connector">Connector</Label>
                  <Select
                    value={
                      customConnector
                        ? "__other__"
                        : connectorByName[connectorName]
                          ? connectorName
                          : ""
                    }
                    onValueChange={(v) => {
                      // Switching connector changes the applicable property set,
                      // so clear any properties entered for the previous one.
                      setGenericProps([]);
                      if (v === "__other__") {
                        setCustomConnector(true);
                        setConnectorName("");
                      } else {
                        setCustomConnector(false);
                        setConnectorName(v);
                      }
                    }}
                  >
                    <SelectTrigger id="cat-connector" className="w-full">
                      <SelectValue placeholder="Select a connector" />
                    </SelectTrigger>
                    <SelectContent>
                      {connectors.map((c) => (
                        <SelectItem key={c.name} value={c.name}>
                          {c.label}
                        </SelectItem>
                      ))}
                      <SelectItem value="__other__">Other (custom)…</SelectItem>
                    </SelectContent>
                  </Select>
                  {customConnector && (
                    <Input
                      value={connectorName}
                      onChange={(e) => setConnectorName(e.target.value)}
                      placeholder="e.g. redshift"
                    />
                  )}
                </div>
                <div className="space-y-1.5">
                  <Label>Properties</Label>
                  <KeyedPropertyList
                    key={customConnector ? "__custom__" : connectorName || "__none__"}
                    catalog={connectorByName[connectorName]?.props ?? []}
                    catalogByKey={Object.fromEntries(
                      (connectorByName[connectorName]?.props ?? []).map((p) => [
                        p.key,
                        p,
                      ]),
                    )}
                    presentKeys={genericProps.map((kv) => kv.key)}
                    getValue={(k) =>
                      genericProps.find((kv) => kv.key === k)?.value ?? ""
                    }
                    setValue={(k, v) =>
                      setGenericProps((prev) => {
                        const rest = prev.filter((kv) => kv.key !== k);
                        return v.trim() === ""
                          ? rest
                          : [...rest, { key: k, value: v }];
                      })
                    }
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
            <Button
              type="submit"
              disabled={addMutation.isPending || !isValid()}
            >
              {addMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
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
          Deleting this engine tears down its Kubernetes resources. This cannot
          be undone.
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
