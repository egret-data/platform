import { useEffect, useState } from "react";
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
  type Engine,
  type EngineCatalog,
  type EngineStatus,
  deleteEngine,
  getEngine,
  listEngineCatalogs,
  restartEngine,
} from "@/api/controlPlane";
import { useControlPlaneCatalogRegistryListRegistryCatalogs } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { controlPlaneCatalogRegistryListRegistryCatalogsResponse } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { CatalogAttachmentDto } from "@/api/generated/model";
import {
  getControlPlaneEnginesListEngineWorkspacesQueryKey,
  useControlPlaneEnginesListEngineWorkspaces,
  useControlPlaneEnginesSetEngineWorkspaces,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import { useAdminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
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
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  AttachmentActions,
  StatusBadge,
  appliedCell,
  useAttachmentActions,
} from "./catalogs/attachmentUi";
import { EngineDependentsTable, dependentsFromError } from "./EngineDependentsTable";
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
import { EnvFromSection } from "./config/ContainersMountsSection";
import { ValidityProvider } from "./config/validityRegistry";
import { ReviewChangesDialog } from "./config/ReviewChangesDialog";
import { EngineAutoscaling } from "./EngineAutoscaling";
import { EngineSecurity } from "./EngineSecurity";
import { RotateSecretsSection } from "./RotateSecretsSection";
import EngineMonitor from "./EngineMonitor";
import PodLogsDialog from "./PodLogsDialog";
import PodTerminalDialog from "./PodTerminalDialog";

/**
 * `GET /engines/{id}/catalogs`, the hand-written query two places on this page
 * read: the Overview strip's catalog count and the Catalogs tab, which needs
 * the `configError` only this route reports. ONE definition, because two
 * literals that drift split the cache silently — both reads keep working and
 * neither sees the other's invalidation.
 */
const engineCatalogsKey = (id: string) => ["cp", "engine-catalogs", id] as const;

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
    case "deleting":
      return "warning";
    default:
      return "secondary";
  }
}

// Low-level transport/plumbing errors the backend can surface verbatim while an
// engine is still coming up (the coordinator/FE isn't reachable yet). These are
// expected churn during provisioning, not something to show an admin raw.
const INTERNAL_ERROR_RE =
  /ServiceError|client error|reqwest|dial tcp|connection refused|connection reset|no route to host|i\/o timeout|\bEOF\b|\(Connect\)/i;

/**
 * Human-facing engine status subtitle. The raw `status.message` can be a
 * low-level transport error (e.g. `ServiceError: client error (Connect)`); during
 * a transient phase that just means "not reachable yet", so we show a
 * phase-appropriate line and keep the raw text on a tooltip for debugging. A
 * genuine failure keeps its message (admins want the detail).
 */
function statusDescription(
  phase: string,
  message: string | null,
): { text: string; title?: string } {
  if (!message) return { text: `Phase: ${phase}` };
  const variant = phaseVariant(phase);
  if (variant === "warning" && INTERNAL_ERROR_RE.test(message)) {
    return { text: "Starting up — the engine isn't reachable yet.", title: message };
  }
  return { text: message };
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
  const [blockedBy, setBlockedBy] = useState<EngineDependentDto[] | null>(null);
  const deleting = engine.status.phase === "deleting";

  const restartMutation = useMutation<void, ApiError, string>({
    mutationFn: restartEngine,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      setRestartOpen(false);
    },
    onError: (e) => toast.error(e.message ?? "Failed to restart the engine."),
  });

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteEngine,
    onSuccess: () => navigate("/control-plane/engines"),
    onError: (e) => {
      // Either branch can leave the engine `deleting` (a 409 here means the
      // cluster was already torn down and a dependent appeared after the
      // preflight check — see the route's step 5): refetch so the header
      // shows the retry state rather than the last live phase.
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engineId] });
      const dependents = dependentsFromError(e);
      if (dependents) {
        setBlockedBy(dependents);
        return;
      }
      toast.error(e.message ?? "Failed to delete the engine.");
    },
  });

  return (
    <>
      {deleting && (
        <Button
          variant="destructive"
          size="sm"
          onClick={() => deleteMutation.mutate(engine.id)}
          disabled={deleteMutation.isPending}
        >
          {deleteMutation.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Trash2 className="h-4 w-4" />
          )}
          Retry delete
        </Button>
      )}

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
        description="This destroys the engine's Kubernetes cluster. It cannot be undone."
        confirmText={engine.name}
        onConfirm={() => deleteMutation.mutate(engine.id)}
      >
        <ul className="list-disc space-y-1 pl-5">
          <li>Query history and notebook engine access on its workspace connections are removed.</li>
          <li>Saved queries that use it lose their connection.</li>
          <li>
            Data API endpoints (including trashed ones), alerts and datasets that use it block
            the delete — you will be shown where they are.
          </li>
        </ul>
      </ConfirmDeleteDialog>

      <AlertDialog
        open={blockedBy !== null}
        onOpenChange={(open) => {
          if (!open) setBlockedBy(null);
        }}
      >
        <AlertDialogContent className="sm:max-w-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>«{engine.name}» is still in use</AlertDialogTitle>
            <AlertDialogDescription>
              Nothing was deleted. Remove or move these first, then delete the engine again.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {blockedBy && <EngineDependentsTable dependents={blockedBy} />}
          <AlertDialogFooter>
            <AlertDialogCancel>Close</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
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

      <div className="flex flex-wrap items-start gap-3">
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
              {data.status.phase === "deleting" ? "Deleting — retry" : data.status.phase}
            </Badge>
            <EngineActions engine={data} engineId={id} />
          </div>
        )}
      </div>

      {engine.isError && !data ? (
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
  useUnsavedChangesWarning(
    isStarRocks ? srCfg.editedSinceLoad : cfg.editedSinceLoad,
  );

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
          <ConfigurationTab
            cfg={cfg}
            imageVersion={engine.spec.imageVersion}
            engineId={engineId}
          />
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
    queryKey: engineCatalogsKey(engine.id),
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
      <div className="overflow-x-auto rounded-md border">
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
      </div>

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
  const d = statusDescription(status.phase, status.message);
  return (
    <section className="space-y-3">
      <div className="space-y-1">
        <h2 className="text-base font-semibold">Status</h2>
        <p className="text-sm text-muted-foreground" title={d.title}>
          {d.text}
        </p>
      </div>
      {status.pods.length === 0 ? (
        <StatusEmptyState phase={status.phase} message={status.message} />
      ) : (
        <PodHealthTable pods={status.pods} engineId={engineId} />
      )}
    </section>
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

      <section className="space-y-3">
        <div className="space-y-1">
          <h2 className="text-base font-semibold">Pods</h2>
          {(() => {
            const d = statusDescription(status.phase, status.message);
            return (
              <p className="text-sm text-muted-foreground" title={d.title}>
                {d.text}
              </p>
            );
          })()}
        </div>
        {status.pods.length === 0 ? (
          <StatusEmptyState phase={status.phase} message={status.message} />
        ) : (
          <PodHealthTable pods={status.pods} engineId={engineId} />
        )}
      </section>

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

/**
 * The Trino config editor: a left-nav of sections + the section panes, wrapped
 * in a single `ValidityProvider`. Shared by the detail Settings tab and the
 * create-engine wizard. Carries NO header or Save/Create button — the consumer
 * supplies those (and reads `onInvalidChange` to gate its action button).
 */
export function TrinoConfigEditor({
  cfg,
  imageVersion,
  onInvalidChange,
  engineId,
}: {
  cfg: EngineConfigForm;
  imageVersion: string;
  onInvalidChange?: (invalid: boolean) => void;
  /**
   * The saved engine's id. Present on the detail Settings tab (enables Rotate
   * secrets); absent in the create wizard, which has nothing to rotate yet.
   */
  engineId?: string;
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

  // Active left-nav section lives in the URL (`?section=`). Normalize unknown/
  // stale values to "image" so a bad deep-link can't render a blank pane.
  const [sp, setSp] = useSearchParams();
  const rawSection = sp.get("section");
  const section = NAV_ITEMS.some((i) => i.key === rawSection)
    ? (rawSection as string)
    : "image";
  const setSection = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("section", v);
        return n;
      },
      { replace: true },
    );

  const countFor = (key: string): number => sectionCounts[key] ?? 0;

  return (
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

      <ValidityProvider onAnyInvalidChange={onInvalidChange ?? (() => {})}>
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
                setContainerSecurityContext={cfg.setContainerSecurityContext}
                deploymentStrategy={cfg.deploymentStrategy}
                setDeploymentStrategy={cfg.setDeploymentStrategy}
                deploymentAnnotations={cfg.deploymentAnnotations}
                setDeploymentAnnotations={cfg.setDeploymentAnnotations}
                revisionHistoryLimit={cfg.revisionHistoryLimit}
                setRevisionHistoryLimit={cfg.setRevisionHistoryLimit}
                progressDeadlineSeconds={cfg.progressDeadlineSeconds}
                setProgressDeadlineSeconds={cfg.setProgressDeadlineSeconds}
                env={cfg.env}
                setEnv={cfg.setEnv}
              />
              <EnvFromSection envFrom={cfg.envFrom} setEnvFrom={cfg.setEnvFrom} />
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
          {section === "security" && (
            <SecuritySection cfg={cfg} engineId={engineId} />
          )}
        </div>
      </ValidityProvider>
    </div>
  );
}

function ConfigurationTab({
  cfg,
  imageVersion,
  engineId,
}: {
  cfg: EngineConfigForm;
  imageVersion: string;
  engineId: string;
}) {
  // Goes true when any raw-JSON override fails to parse; blocks Save.
  const [rawInvalid, setRawInvalid] = useState(false);

  return (
    <section className="space-y-4">
      <div className="space-y-1">
        <h2 className="text-base font-semibold">Settings</h2>
        <p className="text-sm text-muted-foreground">
          Cluster-wide, coordinator, and worker Trino config plus security and exposure. Saving
          re-provisions the engine.
        </p>
      </div>
      <div className="space-y-4">
        <TrinoConfigEditor
          cfg={cfg}
          imageVersion={imageVersion}
          onInvalidChange={setRawInvalid}
          engineId={engineId}
        />
        <SaveBar cfg={cfg} disabled={rawInvalid} review />
      </div>
    </section>
  );
}

/**
 * The Security & Exposure content (client auth + coordinator ingress), extracted
 * from the former standalone Security tab so it can render as the last section of
 * the merged Settings left-nav. It carries NO Card or SaveBar of its own — the
 * single {@link ConfigurationTab} `SaveBar` (same `cfg`) covers security edits too.
 */
function SecuritySection({
  cfg,
  engineId,
}: {
  cfg: EngineConfigForm;
  engineId?: string;
}) {
  const {
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
    delegationMode,
    setDelegationMode,
    storedDelegationMode,
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
    ingressTlsCertSecretRef,
    setIngressTlsCertSecretRef,
    ingressAnnotations,
    setIngressAnnotations,
  } = cfg;

  return (
    <div className="space-y-5">
      <EngineSecurity
        passwordUsers={passwordUsers}
        setPasswordUsers={setPasswordUsers}
        jwtEnabled={jwtEnabled}
        setJwtEnabled={setJwtEnabled}
        delegationMode={delegationMode}
        setDelegationMode={setDelegationMode}
        storedDelegationMode={storedDelegationMode}
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
        ingressTlsCertSecretRef={ingressTlsCertSecretRef}
        setIngressTlsCertSecretRef={setIngressTlsCertSecretRef}
        ingressAnnotations={ingressAnnotations}
        setIngressAnnotations={setIngressAnnotations}
      />
      {engineId && <RotateSecretsSection engineId={engineId} />}
    </div>
  );
}

// ── Catalogs ─────────────────────────────────────────────────────────────────
//
// Config-style master-detail (mirrors ConfigurationTab's left-nav layout):
// the left nav lists catalogs + a sentinel "add" entry; the right pane hosts
// either the add form or an edit form for whichever catalog is selected.

/** Sentinel `selected` value that shows the add-catalog form instead of an
 * existing catalog's detail. */
/**
 * Control Plane → Engines → detail → **Catalogs** (SP3c-1 Task 9).
 *
 * The mirror of `catalogs/EnginesTab.tsx`: the same `(catalog, engine)` pairs,
 * read from the engine's end. It replaced an inline `CatalogForm` master/detail
 * that created and edited catalogs here; catalogs are created and edited in one
 * place now, Control Plane → Catalogs, and this screen attaches, detaches and
 * retries.
 *
 * TWO reads, deliberately, and neither is redundant:
 *
 *   - the **registry list** (`catalog_registry:read`) is the only source of
 *     `applyStatus`, `appliedAt` and the catalogs this engine does NOT have —
 *     the picker;
 *   - the engine's **own** catalog list (`engine:read`, the permission this
 *     page already needs, and the same cache key the Overview strip holds) is
 *     the only route that says a stored configuration no longer parses.
 *     `configError` exists precisely so a row the tightened validator refuses
 *     is findable rather than invisible, and this is the screen that renders it.
 *
 * Because they are gated differently, either can be missing, and the table is
 * their union: rows the registry cannot show still list by name, with the
 * Apply column left honestly empty and the missing permission named.
 */
type AttachedRow = {
  id: string;
  name: string;
  kind: string;
  isPrimary: boolean;
  /** `null` when egret could not read the registry — NOT "no attachment". */
  a: CatalogAttachmentDto | null;
  /** The stored configuration does not parse, and this is why. */
  configError: string | null;
};

function CatalogsCard({ engine }: { engine: EngineWithStatus }) {
  const { me } = useAuth();
  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  const canUpdate = isAdmin || !!me?.permissions.includes("catalog_registry:update");
  // Linking to a catalog's page is `catalog_registry:read`, which `engine:read`
  // does not imply: a link that 403s the persona who holds only the engine keys
  // is worse than a plain name (the rule Task 7's engine link follows).
  const canReadCatalogs = isAdmin || !!me?.permissions.includes("catalog_registry:read");

  const [pick, setPick] = useState("");
  const [detachTarget, setDetachTarget] = useState<AttachedRow | null>(null);

  // `<…, ApiError>`: the thrown value is an `ApiError` with `.message`, and the
  // generated `TError = ErrorBody` would make `.error` type-check and be
  // `undefined` at runtime.
  const registryQ = useControlPlaneCatalogRegistryListRegistryCatalogs<
    controlPlaneCatalogRegistryListRegistryCatalogsResponse,
    ApiError
  >(undefined, { query: { retry: false } });
  const all = registryQ.data?.status === 200 ? registryQ.data.data : [];

  const legacyQ = useQuery<EngineCatalog[], ApiError>({
    queryKey: engineCatalogsKey(engine.id),
    queryFn: () => listEngineCatalogs(engine.id),
    retry: false,
  });

  const queryClient = useQueryClient();
  const { attach, detach, retry } = useAttachmentActions({
    // The registry keys are invalidated by the hook; this engine's own list is
    // this page's, and it carries the `configError` verdict — a detached row
    // that stayed in it would keep its broken badge after it had gone.
    onDone: () => {
      void queryClient.invalidateQueries({ queryKey: engineCatalogsKey(engine.id) });
    },
  });

  const legacyRows = legacyQ.data ?? [];
  /**
   * Whether egret CHECKED. `undefined` data is loading or failed, and "not
   * checked" must not render like "checked and clean": silence where a defect
   * would be shown reads as an all-clear.
   */
  const configChecked = legacyQ.data !== undefined;
  const legacyById = new Map(legacyRows.map((c) => [c.id, c]));

  const rows: AttachedRow[] = [];
  const seen = new Set<string>();
  for (const c of all) {
    const a = c.engines.find((e) => e.engineId === engine.id);
    if (!a) continue;
    seen.add(c.id);
    rows.push({
      id: c.id,
      name: c.name,
      kind: c.kind,
      isPrimary: a.isPrimary,
      a,
      configError: legacyById.get(c.id)?.configError ?? null,
    });
  }
  // Rows the registry did not show. Normally none — both lists come from the
  // same `list_engine_attachments` — so this is the 403 path: the operator
  // keeps the names this page has always shown instead of losing the tab.
  for (const l of legacyRows) {
    if (seen.has(l.id)) continue;
    rows.push({
      id: l.id,
      name: l.name,
      kind: l.kind,
      isPrimary: l.isPrimary,
      a: null,
      configError: l.configError ?? null,
    });
  }

  const candidates = all.filter((c) => !c.engines.some((e) => e.engineId === engine.id));
  const broken = rows.filter((r) => r.configError);
  const applyFailures = rows.filter((r) => r.a?.applyStatus === "failed" && r.a.applyError);
  const busy = retry.isPending || detach.isPending;

  // The brief's "adding a catalog rolls the pods" was true of Trino and false
  // of StarRocks, and the difference is the one an operator plans around.
  const applyProse = isStarRocksType(engine.type)
    ? "Attaching or detaching a catalog creates or drops it on the StarRocks FE in place, without rolling the pods."
    : "Attaching or detaching a catalog applies it to the cluster and restarts the Trino coordinator, so queries in flight are lost.";

  const retryBlockedReason = (r: AttachedRow) =>
    r.configError
      ? "egret cannot parse this catalog's stored configuration, so it cannot render it for this engine. Repair it on the catalog's own page first."
      : null;

  return (
    <section className="space-y-4">
      <div className="space-y-1">
        <h2 className="text-base font-semibold">Catalogs</h2>
        <p className="text-sm text-muted-foreground" data-testid="engine-catalogs-apply-prose">
          Data sources this engine can query. {applyProse}
        </p>
      </div>

      {rows.length === 0 ? (
        legacyQ.isLoading || registryQ.isLoading ? (
          <ListSkeleton rows={3} cols={4} />
        ) : (
          <p className="text-sm text-muted-foreground">
            This engine has no catalogs, so it can query nothing yet.
          </p>
        )
      ) : (
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Catalog</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead>Primary</TableHead>
                {/* "Apply", not "Status": it is the CATALOG's state on this
                    engine and says nothing about the engine, whose own phase
                    is in the page header. */}
                <TableHead>Apply</TableHead>
                <TableHead>Applied</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((r) => (
                <TableRow key={r.id}>
                  <TableCell className="font-medium">
                    <div className="flex flex-wrap items-center gap-2">
                      {canReadCatalogs ? (
                        <Link to={`/control-plane/catalogs/${r.id}`} className="hover:underline">
                          {r.name}
                        </Link>
                      ) : (
                        r.name
                      )}
                      {r.configError && <Badge variant="destructive">config invalid</Badge>}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{r.kind}</Badge>
                  </TableCell>
                  <TableCell>
                    {r.isPrimary ? <Badge variant="secondary">primary</Badge> : null}
                  </TableCell>
                  <TableCell>
                    {r.a ? (
                      <StatusBadge a={r.a} />
                    ) : (
                      <span className="text-sm text-muted-foreground">—</span>
                    )}
                  </TableCell>
                  <TableCell className="text-sm text-muted-foreground">
                    {r.a ? appliedCell(r.a) : "—"}
                  </TableCell>
                  <TableCell className="text-right">
                    {canUpdate && r.a && (
                      <AttachmentActions
                        a={r.a}
                        retryBlockedReason={retryBlockedReason(r)}
                        detachBlockedReason={
                          r.isPrimary
                            ? `${r.name} is this engine's primary catalog. Detach it by deleting the engine.`
                            : null
                        }
                        busy={busy}
                        onRetry={() => retry.mutate({ id: engine.id, cid: r.id })}
                        onDetach={() => setDetachTarget(r)}
                      />
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {rows.some((r) => r.a) && (
        <p className="text-xs text-muted-foreground" data-testid="engine-apply-footnote">
          <strong>Apply</strong> is this catalog&apos;s state on this engine, not a check that it
          still works: a catalog that reached the engine before the engine broke is still recorded
          as applied here. This engine&apos;s own phase is in the header above, and the catalog&apos;s
          own page can test it.
        </p>
      )}

      {/* `applyError` in full, below the table. It is the one string that says
          what to fix, and a `title` attribute is not something an operator can
          read at a glance, copy, or capture in a screenshot. */}
      {applyFailures.length > 0 && (
        <Alert variant="destructive" data-testid="engine-catalog-apply-errors">
          <AlertDescription>
            <ul className="space-y-1">
              {applyFailures.map((r) => (
                <li key={r.id}>
                  <span className="font-medium">{r.name}</span>: {r.a?.applyError}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}

      {/* The `configError` carry. A stored row the tightened validator now
          refuses is listed rather than hidden, and this is what makes it
          actionable: the reason in full, and a link to the page that edits it.
          `retry_apply` and every engine save render this catalog, so both are
          refused until it is repaired — which is a confusing failure on the
          Settings tab unless it is said here. */}
      {broken.length > 0 && (
        <Alert variant="destructive" data-testid="engine-catalog-config-errors">
          <AlertDescription className="space-y-2">
            <p>
              egret cannot parse what it stored for{" "}
              {broken.length === 1 ? "one of this engine's catalogs" : "these catalogs"}. They stay
              attached and the running cluster is untouched, but egret cannot render them for this
              engine — so Retry, and any save on the Settings tab, is refused until they are
              repaired.
            </p>
            <ul className="space-y-1">
              {broken.map((r) => (
                <li key={r.id}>
                  <span className="font-medium">{r.name}</span>: {r.configError}
                  {" — "}
                  {canReadCatalogs ? (
                    <Link to={`/control-plane/catalogs/${r.id}`} className="underline">
                      repair {r.name}
                    </Link>
                  ) : (
                    <>
                      repairing it is done on the catalog&apos;s own page, which needs the
                      catalog_registry:read permission; a platform administrator can grant it.
                    </>
                  )}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}

      {!configChecked && rows.length > 0 && legacyQ.isError && (
        <p className="text-xs text-muted-foreground" data-testid="engine-catalog-config-unchecked">
          egret could not check whether these catalogs&apos; stored configuration still parses:{" "}
          {legacyQ.error?.message ?? "the request failed"}. A row that no longer parses would be
          marked here, so an unmarked row is not evidence that it does.
        </p>
      )}

      {registryQ.isError && (
        <Alert data-testid="engine-catalogs-registry-blocked">
          <AlertDescription>
            {registryQ.error?.status === 403
              ? "Apply status, Retry, Detach and attaching all go through the catalog registry, which requires the catalog_registry:read permission. Your account does not have it, so egret can show you which catalogs this engine has but not their state. A platform administrator can grant catalog_registry:read."
              : `Couldn't read the catalog registry: ${
                  registryQ.error?.message ?? "the request failed"
                }. The catalogs above are the engine's own list; their apply status, Retry, Detach and the attach picker are unavailable until it answers.`}
          </AlertDescription>
        </Alert>
      )}

      {canUpdate && !registryQ.isError && (
        <div className="space-y-2 border-t pt-4">
          <Label htmlFor="engine-attach-catalog">Catalog to attach</Label>
          <div className="flex flex-wrap items-center gap-2">
            <Select value={pick} onValueChange={setPick}>
              <SelectTrigger id="engine-attach-catalog" className="w-72">
                <SelectValue placeholder="Select a catalog" />
              </SelectTrigger>
              <SelectContent>
                {candidates.map((c) => (
                  <SelectItem key={c.id} value={c.id}>
                    {c.name} ({c.kind})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              onClick={() =>
                // The picker clears on SUCCESS only: a failed attach that reset
                // the Select would make the operator find the catalog again to
                // read the error against.
                attach.mutate({ id: pick, engineId: engine.id }, { onSuccess: () => setPick("") })
              }
              disabled={!pick || attach.isPending}
            >
              {attach.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Attach
            </Button>
          </div>
          {!registryQ.isLoading && candidates.length === 0 && (
            <p className="text-xs text-muted-foreground">
              {all.length === 0
                ? "There are no catalogs to attach. Create one in Control Plane → Catalogs."
                : "Every catalog egret can show you is already on this engine."}
            </p>
          )}
        </div>
      )}

      {!canUpdate && (
        <p className="text-xs text-muted-foreground" data-testid="engine-catalogs-readonly">
          Attaching, detaching and re-applying a catalog need the catalog_registry:update
          permission, which this account does not have. A platform administrator can grant it.
        </p>
      )}

      <ConfirmDeleteDialog
        open={detachTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDetachTarget(null);
        }}
        title={
          detachTarget
            ? `Detach «${detachTarget.name}» from «${engine.name}»?`
            : "Detach catalog?"
        }
        description={
          detachTarget
            ? `${engine.name} will no longer serve this catalog, and the credential egret minted for the attachment is revoked. The catalog itself is kept, with its configuration and its other attachments.`
            : ""
        }
        // The catalog's own name, not the word "delete": this removes the
        // catalog from ONE engine, and a dialog that asks for "delete" reads
        // like the destructive operation it is not.
        confirmText={detachTarget?.name ?? ""}
        confirmLabel="Detach"
        onConfirm={() => {
          if (detachTarget) detach.mutate({ id: detachTarget.id, engineId: engine.id });
          setDetachTarget(null);
        }}
      />
    </section>
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

  const [blockedBy, setBlockedBy] = useState<EngineDependentDto[] | null>(null);

  const setMut = useControlPlaneEnginesSetEngineWorkspaces({
    mutation: {
      onSuccess: () => {
        setBlockedBy(null);
        queryClient.invalidateQueries({
          queryKey: getControlPlaneEnginesListEngineWorkspacesQueryKey(engineId),
        });
      },
    },
  });

  // Per call rather than on the hook, so a 409 renders the dependents table and
  // is not ALSO toasted by a hook-level handler.
  const onSetError = (e: unknown) => {
    const dependents = dependentsFromError(e);
    if (dependents) {
      setBlockedBy(dependents);
      return;
    }
    toast.error((e as { message?: string })?.message ?? "Failed to update workspaces.");
  };

  const addWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds, wsId] } },
      { onSuccess: () => toast.success("Workspace assigned"), onError: onSetError },
    );
  const removeWorkspace = (wsId: string) =>
    setMut.mutate(
      { id: engineId, data: { workspaceIds: [...assignedIds].filter((x) => x !== wsId) } },
      { onSuccess: () => toast.success("Workspace removed"), onError: onSetError },
    );

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <h2 className="text-base font-semibold">Workspaces</h2>
          <p className="text-sm text-muted-foreground">
            Workspaces assigned to this engine can query it with per-user delegation.{" "}
            {jwtEnabled
              ? ""
              : "Enable JWT authentication on the Security & Exposure tab before assigning."}
          </p>
        </div>
        <Button
          className="shrink-0"
          size="sm"
          disabled={!jwtEnabled}
          title={jwtEnabled ? undefined : "Engine needs JWT authentication to be assignable"}
          onClick={() => setAddOpen(true)}
        >
          <Plus className="mr-1 h-4 w-4" /> Add workspace
        </Button>
      </div>
      {blockedBy && (
        <div className="space-y-2 rounded-md border border-destructive/50 p-3">
          <p className="text-sm font-medium text-destructive">
            Not removed — the workspace still uses this engine
          </p>
          <p className="text-sm text-muted-foreground">
            Remove or move these first, then try again.
          </p>
          <EngineDependentsTable dependents={blockedBy} />
        </div>
      )}
      <div>
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
      </div>

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
    </section>
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

