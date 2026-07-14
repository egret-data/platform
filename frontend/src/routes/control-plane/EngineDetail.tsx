import { useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, RotateCw, Server, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  type CatalogTestResult,
  type Engine,
  type EngineCatalog,
  type EngineCatalogDetail,
  type EngineStatus,
  deleteEngine,
  deleteEngineCatalog,
  getEngine,
  getEngineCatalog,
  listEngineCatalogs,
  restartEngine,
  testEngineCatalog,
} from "@/api/controlPlane";
import {
  getControlPlaneEnginesListEngineWorkspacesQueryKey,
  useControlPlaneEnginesListEngineWorkspaces,
  useControlPlaneEnginesSetEngineWorkspaces,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import { useAdminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import { ApiError } from "@/api/client";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { CatalogForm } from "./CatalogForm";
import {
  useEngineConfigForm,
  type EngineConfigForm,
} from "./useEngineConfigForm";
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
import PodLogsDialog from "./PodLogsDialog";
import PodTerminalDialog from "./PodTerminalDialog";

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
        <ListSkeleton rows={6} cols={3} />
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
  // Warn before refresh/close while a config or security edit is unsaved — these
  // saves re-provision the engine, so a lost edit is expensive.
  useUnsavedChangesWarning(cfg.dirty);

  // Active top-level tab lives in the URL (`?tab=`) so the view is
  // deep-linkable and survives a refresh. Default preserved: "overview".
  const [sp, setSp] = useSearchParams();
  const tab = sp.get("tab") ?? "overview";
  const setTab = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", v);
        return n;
      },
      { replace: true },
    );

  return (
    <Tabs value={tab} onValueChange={setTab} className="space-y-5">
      <TabsList>
        <TabsTrigger value="overview">Overview</TabsTrigger>
        <TabsTrigger value="catalogs">Catalogs</TabsTrigger>
        <TabsTrigger value="workspaces">Workspaces</TabsTrigger>
        <TabsTrigger value="configuration">Configuration</TabsTrigger>
        <TabsTrigger value="security">Security &amp; Exposure</TabsTrigger>
        <TabsTrigger value="monitor">Monitor</TabsTrigger>
      </TabsList>

      <TabsContent value="overview" className="space-y-5">
        <StatusCard status={engine.status} engineId={engineId} />
        <DangerCard engine={engine} />
      </TabsContent>

      <TabsContent value="catalogs">
        <CatalogsCard engine={engine} />
      </TabsContent>

      <TabsContent value="workspaces">
        <WorkspacesCard engineId={engineId} />
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

function StatusCard({
  status,
  engineId,
}: {
  status: EngineStatus;
  engineId: string;
}) {
  const queryClient = useQueryClient();
  const [logsPod, setLogsPod] = useState<string | null>(null);
  const [execPod, setExecPod] = useState<string | null>(null);
  const [execConfirmPod, setExecConfirmPod] = useState<string | null>(null);
  const [restartOpen, setRestartOpen] = useState(false);

  const restartMutation = useMutation<void, ApiError, string>({
    mutationFn: restartEngine,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      setRestartOpen(false);
    },
  });

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="space-y-1.5">
            <CardTitle className="text-lg">Status</CardTitle>
            <CardDescription>
              {status.message ?? `Phase: ${status.phase}`}
            </CardDescription>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setRestartOpen(true)}
            disabled={restartMutation.isPending}
          >
            {restartMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <RotateCw className="h-4 w-4" />
            )}
            Restart
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Pod</TableHead>
              <TableHead>Phase</TableHead>
              <TableHead>Ready</TableHead>
              <TableHead className="text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {status.pods.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell
                  colSpan={4}
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
                  <TableCell className="text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setLogsPod(pod.name)}
                    >
                      Logs
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setExecConfirmPod(pod.name)}
                    >
                      Exec
                    </Button>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </CardContent>

      <PodLogsDialog
        engineId={engineId}
        pod={logsPod}
        onOpenChange={(o) => !o && setLogsPod(null)}
      />

      <PodTerminalDialog
        engineId={engineId}
        pod={execPod}
        onOpenChange={(o) => !o && setExecPod(null)}
      />

      <AlertDialog
        open={execConfirmPod != null}
        onOpenChange={(o) => !o && setExecConfirmPod(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Open shell into {execConfirmPod}?</AlertDialogTitle>
            <AlertDialogDescription>
              Open an interactive shell into {execConfirmPod}? This runs
              commands inside the running pod.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                setExecPod(execConfirmPod);
                setExecConfirmPod(null);
              }}
            >
              Open shell
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={restartOpen} onOpenChange={setRestartOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Restart engine?</AlertDialogTitle>
            <AlertDialogDescription>
              This rolls the coordinator and worker pods; running queries are
              interrupted.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={(e) => {
                e.preventDefault();
                restartMutation.mutate(engineId);
              }}
              disabled={restartMutation.isPending}
            >
              {restartMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Restart
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
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

  // Active left-nav section lives in the URL (`?section=`), merged alongside the
  // top-level `?tab=`. Default preserved: "image".
  const [sp, setSp] = useSearchParams();
  const section = sp.get("section") ?? "image";
  const setSection = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("section", v);
        return n;
      },
      { replace: true },
    );

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
//
// Config-style master-detail (mirrors ConfigurationTab's left-nav layout):
// the left nav lists catalogs + a sentinel "add" entry; the right pane hosts
// either the add form or an edit form for whichever catalog is selected.

/** Sentinel `selected` value that shows the add-catalog form instead of an
 * existing catalog's detail. */
const ADD_CATALOG = "add";

/**
 * "Test connection" + Remove footer for an existing catalog's detail pane.
 * Rendered with `key={selected}` by the caller so switching catalogs mounts
 * a fresh instance (and thus a fresh mutation) instead of carrying over the
 * previous catalog's test result.
 */
function CatalogTestSection({
  engineId,
  cid,
  onRemove,
}: {
  engineId: string;
  cid: string;
  onRemove: () => void;
}) {
  const testMutation = useMutation<CatalogTestResult, ApiError, void>({
    mutationFn: () => testEngineCatalog(engineId, cid),
  });

  return (
    <div className="space-y-3 border-t pt-4">
      <div className="flex items-center justify-between gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => testMutation.mutate()}
          disabled={testMutation.isPending}
        >
          {testMutation.isPending && (
            <Loader2 className="h-4 w-4 animate-spin" />
          )}
          Test connection
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground"
          onClick={onRemove}
        >
          <Trash2 className="h-4 w-4" />
          Remove
        </Button>
      </div>

      {testMutation.data?.ok && (
        <Alert>
          <AlertDescription className="text-success">
            Connection OK
          </AlertDescription>
        </Alert>
      )}
      {testMutation.data && !testMutation.data.ok && (
        <Alert variant="destructive">
          <AlertDescription>{testMutation.data.message}</AlertDescription>
        </Alert>
      )}
      {testMutation.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {testMutation.error?.message ?? "Test failed."}
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}

function CatalogsCard({ engine }: { engine: EngineWithStatus }) {
  const queryClient = useQueryClient();
  const catalogs = useQuery<EngineCatalog[], ApiError>({
    queryKey: ["cp", "engine-catalogs", engine.id],
    queryFn: () => listEngineCatalogs(engine.id),
    retry: false,
  });
  const rows = catalogs.data ?? [];

  const [selected, setSelected] = useState<string>(ADD_CATALOG);
  const [removeOpen, setRemoveOpen] = useState(false);

  // Tracks whether we've applied the initial "default to the first catalog"
  // selection yet, and the previous set of catalog ids (to detect a
  // newly-added catalog after an add-triggered refetch).
  const didInitRef = useRef(false);
  const prevIdsRef = useRef<Set<string> | null>(null);

  useEffect(() => {
    if (!catalogs.data) return;
    const ids = new Set(catalogs.data.map((c) => c.id));

    if (!didInitRef.current) {
      didInitRef.current = true;
      setSelected(catalogs.data.length > 0 ? catalogs.data[0].id : ADD_CATALOG);
    } else if (selected === ADD_CATALOG) {
      // After adding, prefer selecting the catalog that just appeared.
      const added = prevIdsRef.current
        ? catalogs.data.find((c) => !prevIdsRef.current!.has(c.id))
        : undefined;
      if (added) setSelected(added.id);
    } else if (!ids.has(selected)) {
      // The selected catalog was removed (or is stale) — fall back.
      setSelected(catalogs.data[0]?.id ?? ADD_CATALOG);
    }

    prevIdsRef.current = ids;
  }, [catalogs.data, selected]);

  const invalidateList = () =>
    queryClient.invalidateQueries({
      queryKey: ["cp", "engine-catalogs", engine.id],
    });
  const invalidateDetail = () =>
    queryClient.invalidateQueries({
      queryKey: ["cp", "engine-catalog", engine.id, selected],
    });

  const detail = useQuery<EngineCatalogDetail, ApiError>({
    queryKey: ["cp", "engine-catalog", engine.id, selected],
    queryFn: () => getEngineCatalog(engine.id, selected),
    enabled: selected !== ADD_CATALOG,
    retry: false,
  });

  const removeMutation = useMutation<void, ApiError, string>({
    mutationFn: (cid) => deleteEngineCatalog(engine.id, cid),
    onSuccess: () => {
      invalidateList();
      setRemoveOpen(false);
    },
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Catalogs</CardTitle>
        <CardDescription>
          Data sources this engine can query. Adding, editing, or removing a
          catalog rolls the pods to reload.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {catalogs.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {catalogs.error?.message ?? "Failed to load catalogs."}
            </AlertDescription>
          </Alert>
        ) : !catalogs.data ? (
          <ListSkeleton rows={5} cols={2} />
        ) : (
          <div className="flex flex-col gap-4 md:flex-row">
            <nav className="shrink-0 space-y-1 md:w-56">
              {rows.map((cat) => {
                const active = selected === cat.id;
                return (
                  <button
                    key={cat.id}
                    type="button"
                    onClick={() => setSelected(cat.id)}
                    className={`flex w-full flex-col gap-0.5 rounded-md px-3 py-2 text-left text-sm transition-colors hover:bg-muted ${
                      active ? "bg-muted font-medium" : "text-muted-foreground"
                    }`}
                  >
                    <span className="flex w-full items-center justify-between gap-2">
                      <span className="truncate">{cat.name}</span>
                      <span className="flex shrink-0 gap-1">
                        {cat.isPrimary && (
                          <Badge variant="secondary">primary</Badge>
                        )}
                        {cat.secretRef && (
                          <Badge variant="secondary">secret</Badge>
                        )}
                      </span>
                    </span>
                    <span className="text-xs font-normal text-muted-foreground">
                      {cat.kind}
                    </span>
                  </button>
                );
              })}

              <div className="border-t pt-1">
                <button
                  type="button"
                  onClick={() => setSelected(ADD_CATALOG)}
                  className={`flex w-full items-center gap-1.5 rounded-md px-3 py-2 text-left text-sm transition-colors hover:bg-muted ${
                    selected === ADD_CATALOG
                      ? "bg-muted font-medium"
                      : "text-muted-foreground"
                  }`}
                >
                  <Plus className="h-3.5 w-3.5" />
                  Add catalog
                </button>
              </div>
            </nav>

            <div className="min-w-0 flex-1">
              {selected === ADD_CATALOG ? (
                <CatalogForm
                  mode="add"
                  engineId={engine.id}
                  onSubmitted={invalidateList}
                />
              ) : detail.isError ? (
                <Alert variant="destructive">
                  <AlertDescription>
                    {detail.error?.message ?? "Failed to load catalog."}
                  </AlertDescription>
                </Alert>
              ) : !detail.data ? (
                <ListSkeleton rows={5} cols={2} />
              ) : (
                <div className="space-y-4">
                  <CatalogForm
                    key={selected}
                    mode="edit"
                    engineId={engine.id}
                    initial={detail.data}
                    onSubmitted={() => {
                      invalidateList();
                      invalidateDetail();
                    }}
                  />
                  {!detail.data.isPrimary && (
                    <CatalogTestSection
                      key={selected}
                      engineId={engine.id}
                      cid={selected}
                      onRemove={() => setRemoveOpen(true)}
                    />
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </CardContent>

      <ConfirmDeleteDialog
        open={removeOpen}
        onOpenChange={setRemoveOpen}
        title={`Remove «${detail.data?.name ?? ""}»?`}
        description="This catalog will be removed and the engine will roll to reload. This cannot be undone."
        confirmText="remove"
        onConfirm={() => {
          if (selected !== ADD_CATALOG) removeMutation.mutate(selected);
        }}
      />
    </Card>
  );
}

// ── Workspaces ───────────────────────────────────────────────────────────────
//
// Assign this engine to workspaces. Only JWT-capable engines can be assigned
// (the backend 400s otherwise — the error is surfaced as a toast). A checkbox
// per workspace drives the full desired set, which is PUT wholesale on Save.

function WorkspacesCard({ engineId }: { engineId: string }) {
  const queryClient = useQueryClient();

  const allWsQ = useAdminWorkspacesListWs(undefined, { query: { retry: false } });
  const allWs = allWsQ.data?.status === 200 ? allWsQ.data.data : [];

  const assignedQ = useControlPlaneEnginesListEngineWorkspaces(engineId, {
    query: { retry: false },
  });

  // Local selection, seeded from the assigned set once it loads. `seeded` guards
  // against re-seeding (which would clobber unsaved toggles) on later refetches.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const seededRef = useRef(false);
  useEffect(() => {
    if (seededRef.current) return;
    if (assignedQ.data?.status !== 200) return;
    seededRef.current = true;
    setSelected(new Set(assignedQ.data.data.map((w) => w.workspaceId)));
  }, [assignedQ.data]);

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const setMut = useControlPlaneEnginesSetEngineWorkspaces({
    mutation: {
      onSuccess: () => {
        queryClient.invalidateQueries({
          queryKey: getControlPlaneEnginesListEngineWorkspacesQueryKey(engineId),
        });
        toast.success("Workspace assignments saved");
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  const save = () =>
    setMut.mutate({ id: engineId, data: { workspaceIds: [...selected] } });

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="space-y-1.5">
            <CardTitle className="text-lg">Workspaces</CardTitle>
            <CardDescription>
              Workspaces this engine is assigned to. Only engines with JWT
              authentication enabled can be assigned; saving otherwise returns an
              error.
            </CardDescription>
          </div>
          <Button type="button" onClick={save} disabled={setMut.isPending}>
            {setMut.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
            Save
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {allWsQ.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {(allWsQ.error as unknown as ApiError | null)?.message ??
                "Failed to load workspaces."}
            </AlertDescription>
          </Alert>
        ) : !allWsQ.data || !assignedQ.data ? (
          <ListSkeleton rows={5} cols={2} />
        ) : allWs.length === 0 ? (
          <p className="py-10 text-center text-sm text-muted-foreground">
            No workspaces yet.
          </p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-10">
                  <span className="sr-only">Assigned</span>
                </TableHead>
                <TableHead>Workspace</TableHead>
                <TableHead>Slug</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {allWs.map((ws) => {
                const checked = selected.has(ws.id);
                return (
                  <TableRow
                    key={ws.id}
                    className="cursor-pointer"
                    onClick={() => toggle(ws.id)}
                  >
                    <TableCell onClick={(e) => e.stopPropagation()}>
                      <Checkbox
                        checked={checked}
                        onCheckedChange={() => toggle(ws.id)}
                        aria-label={`Assign to ${ws.name}`}
                      />
                    </TableCell>
                    <TableCell className="font-medium">{ws.name}</TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      {ws.slug}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
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
