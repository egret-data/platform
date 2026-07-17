import { useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Loader2,
  MoreHorizontal,
  Plus,
  RotateCw,
  Server,
  Trash2,
} from "lucide-react";
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
import { SummaryStrip } from "@/components/explore/SummaryStrip";
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
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
import { useStarRocksConfigForm } from "./useStarRocksConfigForm";
import { StarRocksConfigTab } from "./StarRocksConfigTab";
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

const isStarRocksType = (t: string) => t.toLowerCase() === "starrocks";

/**
 * Derive the engine's in-cluster `host:port` endpoint. The engine DTO carries no
 * endpoint field, so we synthesise the namespaced Service DNS name by kind:
 *   - Trino     → `{name}-coordinator.{namespace}:8080`
 *   - StarRocks → `{name}-fe-service.{namespace}:9030` (FE query port)
 */
function engineEndpoint(engine: EngineWithStatus): string {
  const { name, namespace } = engine;
  return isStarRocksType(engine.type)
    ? `${name}-fe-service.${namespace}:9030`
    : `${name}-coordinator.${namespace}:8080`;
}

/** Container image the engine runs (falls back to the raw `imageVersion`). */
function engineImage(engine: EngineWithStatus): string {
  if (isStarRocksType(engine.type)) {
    return engine.spec.starrocksConfig?.feImage ?? engine.spec.imageVersion;
  }
  const v = engine.spec.imageVersion;
  // Trino stores just the tag; render the full ref for clarity unless it already
  // looks like a full `repo:tag` reference.
  return v.includes("/") ? v : `trinodb/trino:${v}`;
}

/** Human-readable auth / delegation summary for the header metadata row. */
function engineAuthLabel(engine: EngineWithStatus): string {
  if (isStarRocksType(engine.type)) {
    const mode = engine.spec.starrocksConfig?.delegation?.mode;
    if (!mode) return "No delegation";
    const m = mode.toLowerCase();
    if (m === "jwt") return "JWT delegation";
    if (m === "impersonate") return "Impersonation";
    return `${mode} delegation`;
  }
  const auth = engine.spec.trinoConfig?.auth;
  if (auth?.jwt) return "JWT auth";
  if (auth?.passwordUsers && auth.passwordUsers.length > 0) return "Password auth";
  return "No auth";
}

/**
 * Muted, `·`-separated identity metadata under the engine name: image, in-cluster
 * endpoint, and auth/delegation mode. Wraps on narrow viewports (no h-scroll).
 */
function EngineHeaderMeta({ engine }: { engine: EngineWithStatus }) {
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
      <span className="truncate">{engineImage(engine)}</span>
      <span aria-hidden className="text-muted-foreground/50">
        ·
      </span>
      <span className="truncate font-mono tabular-nums">
        {engineEndpoint(engine)}
      </span>
      <span aria-hidden className="text-muted-foreground/50">
        ·
      </span>
      <span className="truncate">{engineAuthLabel(engine)}</span>
    </div>
  );
}

/**
 * Page-level engine actions in the header, consistent across every tab:
 * a primary **Restart** (rolls the pods; both engine kinds) and an overflow
 * `⋯` menu whose **Delete engine** item tears the cluster down and returns to
 * the engines list. Both confirm before acting.
 */
function EngineActions({
  engine,
  engineId,
}: {
  engine: EngineWithStatus;
  engineId: string;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [restartOpen, setRestartOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const restartMutation = useMutation<void, ApiError, string>({
    mutationFn: restartEngine,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      setRestartOpen(false);
    },
  });

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteEngine,
    onSuccess: () => navigate("/control-plane/engines"),
  });

  return (
    <>
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

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            className="px-2"
            aria-label="More engine actions"
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            className="cursor-pointer text-destructive focus:text-destructive"
            onClick={() => setDeleteOpen(true)}
          >
            <Trash2 className="h-4 w-4" />
            Delete engine
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

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

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete «${engine.name}»?`}
        description="This engine and its Kubernetes resources (cluster, catalog, secret) will be removed. This cannot be undone."
        confirmText="delete"
        onConfirm={() => deleteMutation.mutate(engine.id)}
      />
    </>
  );
}

/**
 * Control Plane → Engine detail.
 *
 * Live status (phase + pods, polled every 5s), editable resource + Trino config,
 * and multi-catalog management. Page-level actions (Restart, Delete) live in the
 * header so they're consistent across every tab. Config/catalog edits
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
        <div className="flex flex-wrap items-start gap-3 border-b bg-muted/30 p-5">
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
            {data && <EngineHeaderMeta engine={data} />}
          </div>
          {data && (
            <div className="flex shrink-0 items-center gap-2">
              <Badge variant="secondary">{data.type}</Badge>
              <Badge variant={phaseVariant(data.status.phase)}>
                {data.status.phase}
              </Badge>
              <EngineActions engine={data} engineId={id} />
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

/** The valid top-level `?tab=` values (drives normalization of stale/legacy
 * bookmarks so a controlled Radix `<Tabs>` never gets an unmatched value). */
const KNOWN_TABS = new Set([
  "overview",
  "catalogs",
  "workspaces",
  "settings",
  "monitor",
]);

/**
 * Top-level engine tabs: Overview / Catalogs / Workspaces / Settings / Monitor.
 * The config form hook is instantiated here so the merged Settings tab — which
 * folds the former Configuration and Security & Exposure views under one
 * left-nav — shares a single form state (and a single Save action).
 */
function EngineTabs({
  engine,
  engineId,
}: {
  engine: EngineWithStatus;
  engineId: string;
}) {
  const cfg = useEngineConfigForm(engine);
  // Parallel StarRocks config form (Task 5). Instantiated unconditionally (hooks
  // rule); for a Trino engine it seeds from an absent `starrocksConfig` and stays
  // inert. Shared with the future StarRocks Security tab off this one instance.
  const srCfg = useStarRocksConfigForm(engine);

  // StarRocks engines get a StarRocks-shaped Settings tab (image/compute/
  // resources + a Security & Delegation section for delegation + TLS); Trino gets
  // the Trino-shaped one (image/server/roles + a Security & Exposure section).
  // Each branch folds config + security under one left-nav off a single form
  // instance (`srCfg` / `cfg`) so one Save covers both.
  const isStarRocks = engine.type === "starrocks";

  // Warn before refresh/close while a config or security edit is unsaved — these
  // saves re-provision the engine, so a lost edit is expensive.
  useUnsavedChangesWarning(isStarRocks ? srCfg.dirty : cfg.dirty);

  // Active top-level tab lives in the URL (`?tab=`) so the view is
  // deep-linkable and survives a refresh. Default preserved: "overview".
  const [sp, setSp] = useSearchParams();
  // Normalize the param: an unknown value on a controlled Radix <Tabs> matches no
  // trigger → blank page. The legacy `configuration`/`security` tab values (now
  // merged) map to "settings"; anything else unknown falls back to "overview".
  const raw = sp.get("tab");
  const tab =
    raw && KNOWN_TABS.has(raw)
      ? raw
      : raw === "configuration" || raw === "security"
        ? "settings"
        : "overview";
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
        <TabsTrigger value="settings">Settings</TabsTrigger>
        <TabsTrigger value="monitor">Monitor</TabsTrigger>
      </TabsList>

      <TabsContent value="overview" className="space-y-4">
        <EngineSummaryStrip engine={engine} />
        <StatusCard status={engine.status} engineId={engineId} />
      </TabsContent>

      <TabsContent value="catalogs">
        <CatalogsCard engine={engine} />
      </TabsContent>

      <TabsContent value="workspaces">
        <WorkspacesCard engine={engine} />
      </TabsContent>

      <TabsContent value="settings">
        {isStarRocks ? (
          <StarRocksConfigTab cfg={srCfg} />
        ) : (
          <ConfigurationTab cfg={cfg} imageVersion={engine.spec.imageVersion} />
        )}
      </TabsContent>

      <TabsContent value="monitor">
        {/* The Trino query monitor (cluster info + running/queued queries + kill)
            has no StarRocks equivalent — StarRocks exposes no such query API. So
            StarRocks gets a pod/cluster health view built from the k8s status we
            already poll (no new API calls); every other kind (incl. unknown/
            future kinds) defaults to the Trino query monitor. Polarity matches
            the Settings tab (keys on `starrocks`, else Trino). */}
        {engine.type === "starrocks" ? (
          <StarRocksMonitor engine={engine} engineId={engineId} />
        ) : (
          <EngineMonitor engineId={engineId} />
        )}
      </TabsContent>
    </Tabs>
  );
}

/**
 * The subset of a config-form hook the {@link SaveBar} reads. Both
 * `useEngineConfigForm` (Trino) and `useStarRocksConfigForm` (StarRocks) satisfy
 * it, so the footer is shared verbatim across both Configuration tabs. The two
 * `*TrinoConfig` fields are only consumed when `review` is set (Trino only).
 */
export interface SaveBarForm {
  error: string | null;
  saved: boolean;
  dirty: boolean;
  saving: boolean;
  save: () => void;
  storedTrinoConfig?: import("./useEngineConfigForm").EngineConfigForm["storedTrinoConfig"];
  builtTrinoConfig?: import("./useEngineConfigForm").EngineConfigForm["builtTrinoConfig"];
}

/**
 * Shared "Save changes" footer for the Configuration and Security tabs. Both
 * act on the same `useEngineConfigForm` instance passed down as `cfg`.
 */
export function SaveBar({
  cfg,
  disabled = false,
  review = false,
}: {
  cfg: SaveBarForm;
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
              after={cfg.builtTrinoConfig ?? {}}
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

/**
 * At-a-glance KPI strip for the Overview tab — engine phase, pod readiness,
 * role replicas, catalog + workspace counts, and the auth/delegation mode.
 * Reuses the control-plane {@link SummaryStrip}; the replica item adapts to the
 * engine kind (StarRocks FE/CN vs Trino coordinator/worker). The two counts
 * reuse the existing Catalogs- and Workspaces-tab list queries (shared cache)
 * and fall back to `—` while those queries load.
 */
function EngineSummaryStrip({ engine }: { engine: EngineWithStatus }) {
  // Same query key as CatalogsCard → shares the cache; we read the count only.
  const catalogsQ = useQuery<EngineCatalog[], ApiError>({
    queryKey: ["cp", "engine-catalogs", engine.id],
    queryFn: () => listEngineCatalogs(engine.id),
    retry: false,
  });
  // Same hook as WorkspacesCard → shares the assignment query; count only.
  const workspacesQ = useControlPlaneEnginesListEngineWorkspaces(engine.id, {
    query: { retry: false },
  });

  const { pods } = engine.status;
  const readyPods = pods.filter((p) => p.ready).length;

  // Both branches defend every replica field: these come off the engine DTO,
  // which may omit them, so a nullish (not falsy — a real 0 must survive)
  // fallback keeps the KPI from rendering the literal "undefined".
  const sr = engine.spec.starrocksConfig;
  const num = (n: number | undefined) => n ?? "—";
  const replicas = isStarRocksType(engine.type)
    ? `FE ${num(sr?.feReplicas)} · CN ${num(sr?.cnReplicas)}`
    : `Coord ${num(engine.spec.coordinatorReplicas)} · Worker ${num(engine.spec.workerReplicas)}`;

  const catalogCount = catalogsQ.data?.length ?? "—";
  const workspaceCount =
    workspacesQ.data?.status === 200 ? workspacesQ.data.data.length : "—";

  const items: { label: string; value: string | number }[] = [
    { label: "phase", value: engine.status.phase },
    { label: "pods", value: `${readyPods}/${pods.length}` },
    { label: "replicas", value: replicas },
    { label: "catalogs", value: catalogCount },
    { label: "workspaces", value: workspaceCount },
    { label: "auth", value: engineAuthLabel(engine) },
  ];

  return <SummaryStrip items={items} />;
}

/**
 * Phase-aware empty state for the pods table (no pods reported yet). The copy
 * and affordance track {@link phaseVariant}:
 *   - warning phases (provisioning/pending/creating/updating) → a skeleton +
 *     "waiting for the operator" reassurance (pods are on their way);
 *   - destructive phases (failed/error) → the operator's `message` in a
 *     destructive Alert, or a generic fallback when none was reported;
 *   - otherwise (running/ready with 0 pods, or an unknown phase) → a neutral line.
 */
function StatusEmptyState({
  phase,
  message,
}: {
  phase: string;
  message: string | null;
}) {
  const variant = phaseVariant(phase);

  if (variant === "warning") {
    return (
      <div className="space-y-3 py-1">
        <p className="text-sm text-muted-foreground">
          Waiting for the operator to create pods…
        </p>
        <ListSkeleton rows={3} cols={3} />
      </div>
    );
  }

  if (variant === "destructive") {
    return (
      <Alert variant="destructive">
        <AlertDescription>
          {message ?? "The engine failed to start."}
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <p className="py-6 text-center text-sm text-muted-foreground">
      No pods reported.
    </p>
  );
}

/**
 * A pod health table — one row per pod (name · phase · ready) with per-pod
 * **Logs** / **Exec** actions — plus the three dialogs those actions drive
 * ({@link PodLogsDialog}, {@link PodTerminalDialog}, and the Exec-confirm
 * `AlertDialog`) and their local state. Shared verbatim by the Overview
 * {@link StatusCard} and the StarRocks {@link StarRocksMonitor} so the two never
 * drift.
 *
 * Renders the table only — callers own the empty state (they branch to
 * {@link StatusEmptyState} when `pods` is empty), so this is never asked to
 * render an empty table. The pod DTO carries only `{ name, phase, ready }`
 * (no restart-count/age), so those columns are intentionally absent.
 */
function PodHealthTable({
  pods,
  engineId,
}: {
  pods: EngineStatus["pods"];
  engineId: string;
}) {
  const [logsPod, setLogsPod] = useState<string | null>(null);
  const [execPod, setExecPod] = useState<string | null>(null);
  const [execConfirmPod, setExecConfirmPod] = useState<string | null>(null);

  return (
    <>
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
          {pods.map((pod) => (
            <TableRow
              key={pod.name}
              className="cursor-default transition-colors hover:bg-muted/50 motion-reduce:transition-none"
            >
              <TableCell className="font-mono text-xs tabular-nums">
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
          ))}
        </TableBody>
      </Table>

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
    </>
  );
}

function StatusCard({
  status,
  engineId,
}: {
  status: EngineStatus;
  engineId: string;
}) {
  return (
    <Card>
      <CardHeader>
        <div className="space-y-1.5">
          <CardTitle className="text-lg">Status</CardTitle>
          <CardDescription>
            {status.message ?? `Phase: ${status.phase}`}
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        {status.pods.length === 0 ? (
          <StatusEmptyState phase={status.phase} message={status.message} />
        ) : (
          <PodHealthTable pods={status.pods} engineId={engineId} />
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Control Plane → Engine → Monitor tab, StarRocks branch.
 *
 * StarRocks exposes no live-query REST API, so the Trino {@link EngineMonitor}
 * (running/queued queries + stage tree + kill) has no equivalent here. Instead
 * we render a **pod/cluster health** view built entirely from the k8s status
 * (`engine.status`) and spec (`engine.spec`) the detail query already polls
 * every 5s — this component issues **no new API calls**.
 *
 * Shows: a cluster summary (phase · pod readiness · FE/CN replicas, plus a CN
 * autoscaling line when configured), a per-pod health table (name · phase ·
 * ready, with Logs/Exec per pod reusing {@link PodLogsDialog}/
 * {@link PodTerminalDialog}), the phase-aware {@link StatusEmptyState} while no
 * pods exist yet, and an honest "query-level monitoring coming soon" footer.
 *
 * The per-pod table + its Logs/Exec dialogs are the shared
 * {@link PodHealthTable} (also used by the Overview {@link StatusCard}).
 */
function StarRocksMonitor({
  engine,
  engineId,
}: {
  engine: EngineWithStatus;
  engineId: string;
}) {
  const { status, spec } = engine;
  const sr = spec.starrocksConfig;
  const num = (n: number | undefined) => n ?? "—";
  const readyPods = status.pods.filter((p) => p.ready).length;

  // CN autoscaling is active only when configured AND maxReplicas > 0
  // (`{ maxReplicas: 0 }` is the stored "disabled" sentinel).
  const autoscaling =
    sr?.cnAutoscaling && sr.cnAutoscaling.maxReplicas > 0
      ? sr.cnAutoscaling
      : null;

  return (
    <div className="space-y-5">
      <div className="space-y-1.5">
        <SummaryStrip
          items={[
            { label: "phase", value: status.phase },
            { label: "pods", value: `${readyPods}/${status.pods.length}` },
            { label: "FE replicas", value: num(sr?.feReplicas) },
            { label: "CN replicas", value: num(sr?.cnReplicas) },
          ]}
        />
        {autoscaling && (
          <p className="text-xs text-muted-foreground">
            Autoscaling: {autoscaling.minReplicas ?? 0}–{autoscaling.maxReplicas}
            {autoscaling.targetCpuPercent != null
              ? ` @ ${autoscaling.targetCpuPercent}% CPU`
              : ""}
          </p>
        )}
      </div>

      <Card>
        <CardHeader>
          <div className="space-y-1.5">
            <CardTitle className="text-lg">Pods</CardTitle>
            <CardDescription>
              {status.message ?? `Phase: ${status.phase}`}
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent>
          {status.pods.length === 0 ? (
            <StatusEmptyState phase={status.phase} message={status.message} />
          ) : (
            <PodHealthTable pods={status.pods} engineId={engineId} />
          )}
        </CardContent>
      </Card>

      <p className="text-xs text-muted-foreground">
        Query-level monitoring for StarRocks is coming soon.
      </p>
    </div>
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
  // Security & Exposure was its own tab; now the LAST config-nav item, sharing
  // the single form + Save below.
  { key: "security", label: "Security & Exposure" },
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
          <CardTitle className="text-lg">Settings</CardTitle>
          <CardDescription>
            Cluster-wide, coordinator, and worker Trino config plus security and
            exposure. Saving re-provisions the engine.
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
                {section === "security" && <SecuritySection cfg={cfg} />}
              </div>
            </ValidityProvider>
          </div>

          <SaveBar cfg={cfg} disabled={rawInvalid} review />
        </div>
      </CardContent>
    </Card>
  );
}

/**
 * The Security & Exposure content (client auth + coordinator ingress), extracted
 * from the former standalone Security tab so it can render as the last section of
 * the merged Settings left-nav. It carries NO Card or SaveBar of its own — the
 * single {@link ConfigurationTab} `SaveBar` (same `cfg`) covers security edits too.
 */
function SecuritySection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
    delegationMode,
    setDelegationMode,
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
    <EngineSecurity
      passwordUsers={passwordUsers}
      setPasswordUsers={setPasswordUsers}
      jwtEnabled={jwtEnabled}
      setJwtEnabled={setJwtEnabled}
      delegationMode={delegationMode}
      setDelegationMode={setDelegationMode}
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

function WorkspacesCard({ engine }: { engine: EngineWithStatus }) {
  const queryClient = useQueryClient();
  const engineId = engine.id;
  const jwtEnabled = engineHasJwt(engine);

  const allWsQ = useAdminWorkspacesListWs(undefined, { query: { retry: false } });
  const allWs = allWsQ.data?.status === 200 ? allWsQ.data.data : [];

  const assignedQ = useControlPlaneEnginesListEngineWorkspaces(engineId, {
    query: { retry: false },
  });
  const assigned = assignedQ.data?.status === 200 ? assignedQ.data.data : [];
  const assignedIds = new Set(assigned.map((w) => w.workspaceId));
  const [addOpen, setAddOpen] = useState(false);

  const setMut = useControlPlaneEnginesSetEngineWorkspaces({
    mutation: {
      onSuccess: () => {
        queryClient.invalidateQueries({
          queryKey: getControlPlaneEnginesListEngineWorkspacesQueryKey(engineId),
        });
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  const addWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds, wsId] } },
      { onSuccess: () => toast.success("Workspace assigned") },
    );
  const removeWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds].filter((x) => x !== wsId) } },
      { onSuccess: () => toast.success("Workspace removed") },
    );

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-1.5">
            <CardTitle className="text-lg">Workspaces</CardTitle>
            <CardDescription>
              Workspaces assigned to this engine can query it with per-user
              delegation.{" "}
              {jwtEnabled
                ? ""
                : "Enable JWT authentication on the Security & Exposure tab before assigning."}
            </CardDescription>
          </div>
          <Button
            size="sm"
            disabled={!jwtEnabled}
            title={jwtEnabled ? undefined : "Engine needs JWT authentication to be assignable"}
            onClick={() => setAddOpen(true)}
          >
            <Plus className="mr-1 h-4 w-4" /> Add workspace
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {assignedQ.isError ? (
          <Alert variant="destructive">
            <AlertDescription>Failed to load workspace assignments.</AlertDescription>
          </Alert>
        ) : !assignedQ.data ? (
          <ListSkeleton rows={3} cols={2} />
        ) : assigned.length === 0 ? (
          <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
            No workspaces assigned to this engine yet.
          </p>
        ) : (
          <div className="space-y-2">
            {assigned.map((w) => (
              <div
                key={w.workspaceId}
                className="flex flex-wrap items-center justify-between gap-3 rounded-md border p-3"
              >
                <div className="min-w-0">
                  <div className="truncate text-sm font-medium">{w.name}</div>
                  <code className="font-mono text-xs text-muted-foreground">{w.slug}</code>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  className="text-destructive hover:text-destructive"
                  disabled={setMut.isPending}
                  onClick={() => removeWorkspace(w.workspaceId)}
                >
                  <Trash2 className="mr-1 h-4 w-4" /> Remove
                </Button>
              </div>
            ))}
          </div>
        )}
      </CardContent>

      <AddWorkspaceDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        workspaces={allWs}
        assignedIds={assignedIds}
        loading={allWsQ.isLoading}
        onAdd={(id) => {
          addWorkspace(id);
          setAddOpen(false);
        }}
      />
    </Card>
  );
}

/** An engine is assignable only when its Trino spec enables JWT auth (mirrors the
 * backend guard on `spec.trinoConfig.auth.jwt`). */
function engineHasJwt(e: EngineWithStatus): boolean {
  return e.spec.trinoConfig?.auth?.jwt != null;
}

/** Searchable picker of ACTIVE, not-yet-assigned workspaces — clicking a row
 * assigns it immediately (mirrors the workspace-side "Assign engine" dialog). */
function AddWorkspaceDialog({
  open,
  onOpenChange,
  workspaces,
  assignedIds,
  loading,
  onAdd,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaces: WorkspaceDto[];
  assignedIds: Set<string>;
  loading: boolean;
  onAdd: (id: string) => void;
}) {
  const [q, setQ] = useState("");
  useEffect(() => {
    if (open) setQ("");
  }, [open]);
  const candidates = workspaces
    .filter((ws) => ws.status === "active" && !assignedIds.has(ws.id))
    .filter((ws) => `${ws.name} ${ws.slug}`.toLowerCase().includes(q.trim().toLowerCase()))
    .sort((a, b) => a.name.localeCompare(b.name) || a.slug.localeCompare(b.slug));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add workspace</DialogTitle>
          <DialogDescription>Assign this engine to an active workspace.</DialogDescription>
        </DialogHeader>
        <Input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Search workspaces…"
          aria-label="Search workspaces"
          autoFocus
        />
        <div className="mt-2 max-h-72 overflow-y-auto rounded-md border">
          {loading ? (
            <p className="p-3 text-sm text-muted-foreground">Loading workspaces…</p>
          ) : candidates.length === 0 ? (
            <p className="p-4 text-sm text-muted-foreground">
              {q
                ? "No workspaces match your search."
                : "All active workspaces are already assigned."}
            </p>
          ) : (
            <ul className="divide-y">
              {candidates.map((ws) => (
                <li key={ws.id}>
                  <button
                    type="button"
                    onClick={() => onAdd(ws.id)}
                    className="flex w-full items-center justify-between gap-3 px-3 py-2 text-left hover:bg-accent"
                  >
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium">{ws.name}</span>
                      <code className="font-mono text-xs text-muted-foreground">{ws.slug}</code>
                    </span>
                    <Plus className="h-4 w-4 shrink-0 text-muted-foreground" />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

