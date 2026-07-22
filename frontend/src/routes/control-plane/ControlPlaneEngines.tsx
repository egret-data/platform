import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Loader2, Plus, Server } from "lucide-react";

import {
  type CreateEngineInput,
  type Engine,
  type Secret,
  type Warehouse,
  createEngine,
  getEngine,
  listEngines,
  listSecrets,
  listWarehouses,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  FilterableTable,
  type Column,
} from "@/components/explore/FilterableTable";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

/**
 * Control Plane → Engines
 *
 * Platform-admin registry of query engines. Trino and StarRocks clusters can
 * both be provisioned on Kubernetes; RisingWave is a placeholder for a later
 * phase. Lists engines per type, shows a live health badge (per-row status
 * poll), and provisions a new engine against a catalog warehouse — the active
 * tab picks the engine kind for the Add dialog.
 */

type EngineType = {
  value: string;
  label: string;
};

const ENGINE_TYPES: EngineType[] = [
  { value: "trino", label: "Trino" },
  { value: "starrocks", label: "StarRocks" },
  { value: "risingwave", label: "RisingWave" },
];

// Trino and StarRocks provisioning are wired up; RisingWave is not yet.
const PROVISIONABLE = new Set(["trino", "starrocks"]);

// StarRocks image defaults (shared-data FE/CN), same versions the backend
// falls back to when the body omits them. Pinned to 4.1.x: it has BOTH the FE
// MySQL-protocol TLS listener (delegation "Enable TLS", open-source >= 3.4.1)
// AND JWT authentication (open-source >= 3.5) that per-user delegation needs —
// 3.4 lacks the authentication_jwt plugin, so JWT logins can't authenticate.
export const DEFAULT_SR_FE_IMAGE = "starrocks/fe-ubuntu:4.1-latest";
export const DEFAULT_SR_CN_IMAGE = "starrocks/cn-ubuntu:4.1-latest";

// Harmless defaults for the still-required Trino-only body fields
// (coordinatorReplicas/workerReplicas/cpuMin/cpuMax/memory) when creating a
// StarRocks engine — the backend ignores them for `kind: "starrocks"` but the
// body validates them as required regardless of kind. Mirrors the Trino
// form's own defaults below.
export const TRINO_FIELD_DEFAULTS = {
  coordinatorReplicas: 1,
  workerReplicas: 2,
  cpuMin: "1",
  cpuMax: "2",
  memory: "4Gi",
};

/** DNS name of the engine's coordinator/FE service, as shown in the Endpoint column. */
function engineEndpoint(engine: Engine): string {
  const suffix = engine.type === "starrocks" ? "-fe-service" : "-coordinator";
  return `${engine.name}${suffix}.${engine.namespace}`;
}

// StarRocks per-user delegation mode (Access parity slice 3 + default-on
// delegation, Task 4). `""` (Auto) is the DEFAULT — omitted from the body so
// the backend resolves jwt→impersonate gracefully. `"jwt"`/`"impersonate"`
// mirror Trino's two delegation modes (see `EngineSecurity.tsx`'s
// `DelegationMode`), applied to the FE JWT integration / service user the
// backend provisions (`create_starrocks_engine`). `"none"` is an EXPLICIT
// open-engine opt-out — sent literally, never omitted.
type StarRocksDelegationOption = "" | "jwt" | "impersonate" | "none";

export const DELEGATION_HELP: Record<StarRocksDelegationOption, string> = {
  "": "Recommended — per-user JWT when the platform supports it, otherwise impersonation.",
  jwt: "Each query uses the caller's own short-lived JWT, bound to their identity over TLS. No shared credential. OIDC users only.",
  impersonate:
    "egret connects as a trusted service account and runs the query as the caller. Works for any signed-in user; runs over TLS.",
  none: "No delegation — the engine's own configured credential is used for all queries.",
};

export const enginesQueryKey = ["cp", "engines"] as const;
const warehousesQueryKey = ["cp", "warehouses"] as const;

export default function ControlPlaneEngines() {
  const engines = useQuery<Engine[], ApiError>({
    queryKey: enginesQueryKey,
    queryFn: listEngines,
    retry: false,
  });

  const [activeType, setActiveType] = useState(ENGINE_TYPES[0].value);
  const [addOpen, setAddOpen] = useState(false);

  const canProvision = PROVISIONABLE.has(activeType);
  const activeLabel =
    ENGINE_TYPES.find((t) => t.value === activeType)?.label ?? activeType;

  const all = engines.data ?? [];
  const total = all.length;
  const running = all.filter((e) => (e.status ?? "").toLowerCase() === "running").length;

  return (
    <div className="space-y-4" data-testid="engines-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <h1 className="text-2xl font-semibold leading-tight">Engines</h1>
          <p className="text-sm text-muted-foreground">
            Platform query engines. Provision a Trino or StarRocks cluster against a catalog
            warehouse and monitor its health. RisingWave arrives in a later phase.
          </p>
        </div>
        <Button
          className="shrink-0"
          onClick={() => setAddOpen(true)}
          disabled={!canProvision}
          title={canProvision ? undefined : "Coming soon"}
        >
          <Plus className="h-4 w-4" />
          Add engine
        </Button>
      </div>

      <SummaryStrip
        items={[
          { label: "engines", value: total },
          { label: "running", value: running },
          { label: "not running", value: total - running },
        ]}
      />

      {engines.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load engines. Please try again.</AlertDescription>
        </Alert>
      ) : (
        <Tabs value={activeType} onValueChange={setActiveType}>
          <TabsList>
            {ENGINE_TYPES.map((type) => (
              <TabsTrigger key={type.value} value={type.value}>
                {type.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {ENGINE_TYPES.map((type) => (
            <TabsContent key={type.value} value={type.value} className="mt-4">
              <EngineTable
                label={type.label}
                rows={(engines.data ?? []).filter((e) => e.type === type.value)}
                loading={engines.isLoading}
                onAdd={PROVISIONABLE.has(type.value) ? () => setAddOpen(true) : undefined}
              />
            </TabsContent>
          ))}
        </Tabs>
      )}

      <AddEngineDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        engineType={activeType}
        engineLabel={activeLabel}
      />
    </div>
  );
}

// Paginate long engine lists: the platform can hold hundreds of engines, and
// each rendered row mounts a live status poll (`EngineStatusBadge`), so
// capping the visible rows also caps the polling fan-out.
const ENGINES_PAGE_SIZE = 25;

function EngineTable({
  label,
  rows,
  loading,
  onAdd,
}: {
  label: string;
  rows: Engine[];
  loading: boolean;
  onAdd?: () => void;
}) {
  if (loading) return <ListSkeleton rows={6} cols={5} />;

  const columns: Column<Engine>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (e) => e.name,
      cell: (e) => (
        <Link
          to={`/control-plane/engines/${e.id}`}
          className="text-primary hover:underline"
        >
          {e.name}
        </Link>
      ),
    },
    {
      key: "type",
      label: "Type",
      value: (e) => e.type,
      cell: (e) => <Badge variant="secondary">{e.type}</Badge>,
    },
    {
      key: "endpoint",
      label: "Endpoint",
      value: (e) => engineEndpoint(e),
      cell: (e) => (
        <span
          className="block max-w-[24rem] truncate font-mono text-xs text-muted-foreground"
          title={engineEndpoint(e)}
        >
          {engineEndpoint(e)}
        </span>
      ),
    },
    {
      key: "status",
      label: "Status",
      cell: (e) => <EngineStatusBadge id={e.id} fallback={e.status} />,
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (e) => (
        <Button variant="ghost" size="sm" asChild>
          <Link
            to={`/control-plane/engines/${e.id}`}
            aria-label={`View ${e.name}`}
          >
            <ExternalLink className="h-4 w-4" />
            Details
          </Link>
        </Button>
      ),
    },
  ];

  return (
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(e) => e.id}
      searchPlaceholder={`Search ${label} engines…`}
      initialSort={{ key: "name", dir: "asc" }}
      pageSize={ENGINES_PAGE_SIZE}
      emptyText={
        <EmptyState
          icon={Server}
          title={`No ${label} engines yet`}
          // No CTA button here: the page header already shows a persistent
          // "Add engine" action, so a second one would be redundant (and the
          // wording adapts to whether this engine kind is provisionable yet).
          description={
            onAdd
              ? 'Use "Add engine" above to provision a cluster against a catalog warehouse.'
              : "Support for this engine arrives in a later phase."
          }
        />
      }
    />
  );
}

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

/** Per-row live health: polls `getEngine` and paints `status.phase` as a badge. */
function EngineStatusBadge({ id, fallback }: { id: string; fallback: string }) {
  const detail = useQuery({
    queryKey: ["cp", "engine", id],
    queryFn: () => getEngine(id),
    refetchInterval: 5000,
    retry: false,
  });

  const phase = detail.data?.status.phase ?? fallback;
  return <Badge variant={phaseVariant(phase)}>{phase}</Badge>;
}

function AddEngineDialog({
  open,
  onOpenChange,
  engineType,
  engineLabel,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  engineType: string;
  engineLabel: string;
}) {
  const queryClient = useQueryClient();
  const warehouses = useQuery<Warehouse[], ApiError>({
    queryKey: warehousesQueryKey,
    queryFn: listWarehouses,
    retry: false,
    enabled: open,
  });
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["control-plane", "secrets"],
    queryFn: listSecrets,
    retry: false,
    enabled: open,
  });
  const createMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: createEngine,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: enginesQueryKey }),
  });

  const [name, setName] = useState("");
  const [warehouseId, setWarehouseId] = useState("");
  const [coordinatorReplicas, setCoordinatorReplicas] = useState(1);
  const [workerReplicas, setWorkerReplicas] = useState(2);
  const [cpuMin, setCpuMin] = useState("1");
  const [cpuMax, setCpuMax] = useState("2");
  const [memory, setMemory] = useState("4Gi");
  // StarRocks-only fields (shown instead of the Trino resource fields above).
  const [feImage, setFeImage] = useState(DEFAULT_SR_FE_IMAGE);
  const [cnImage, setCnImage] = useState(DEFAULT_SR_CN_IMAGE);
  const [feReplicas, setFeReplicas] = useState(1);
  const [cnReplicas, setCnReplicas] = useState(1);
  const [delegationMode, setDelegationMode] =
    useState<StarRocksDelegationOption>("");
  const [tlsEnabled, setTlsEnabled] = useState(true);
  // Optional coordinator TLS cert: "" = self-signed (default); otherwise the id
  // of a kubernetes.io/tls named secret.
  const [tlsCertSecretRef, setTlsCertSecretRef] = useState("");
  const [error, setError] = useState<string | null>(null);

  const isStarRocks = engineType === "starrocks";

  useEffect(() => {
    if (open) reset();
  }, [open]);

  function reset() {
    setName("");
    setWarehouseId("");
    setCoordinatorReplicas(1);
    setWorkerReplicas(2);
    setCpuMin("1");
    setCpuMax("2");
    setMemory("4Gi");
    setFeImage(DEFAULT_SR_FE_IMAGE);
    setCnImage(DEFAULT_SR_CN_IMAGE);
    setFeReplicas(1);
    setCnReplicas(1);
    setDelegationMode("");
    setTlsEnabled(true);
    setTlsCertSecretRef("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  // Only provisioned warehouses (embedded catalog id resolved) can back an engine.
  const provisioned = (warehouses.data ?? []).filter(
    (w) => w.lakekeeperWarehouseId,
  );

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const input: CreateEngineInput = isStarRocks
      ? {
          name: name.trim(),
          warehouseId,
          kind: "starrocks",
          starrocks: {
            feImage: feImage.trim(),
            cnImage: cnImage.trim(),
            feReplicas,
            cnReplicas,
            // Omit entirely for Auto ("") — the backend resolves an absent
            // delegationMode to jwt, degrading to impersonate. An explicit
            // "none" is sent LITERALLY so the backend provisions an open
            // engine instead of applying the Auto default.
            ...(delegationMode !== ""
              ? { delegationMode, tlsEnabled }
              : {}),
          },
          // Required by the body but ignored server-side for StarRocks —
          // send the same defaults the Trino wizard uses.
          ...TRINO_FIELD_DEFAULTS,
        }
      : {
          name: name.trim(),
          warehouseId,
          coordinatorReplicas,
          workerReplicas,
          cpuMin: cpuMin.trim(),
          cpuMax: cpuMax.trim(),
          memory: memory.trim(),
          ...(tlsCertSecretRef ? { tlsCertSecretRef } : {}),
        };

    try {
      await createMutation.mutateAsync(input);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to provision engine.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add {engineLabel} engine</DialogTitle>
            <DialogDescription>
              Provision a {engineLabel} cluster against a catalog warehouse.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpe-name">Name</Label>
              <Input
                id="cpe-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. analytics-trino"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpe-warehouse">Warehouse</Label>
              <Select value={warehouseId} onValueChange={setWarehouseId}>
                <SelectTrigger id="cpe-warehouse" className="w-full">
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

            {isStarRocks ? (
              <>
                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-sr-fe-image">FE image</Label>
                    <Input
                      id="cpe-sr-fe-image"
                      value={feImage}
                      onChange={(e) => setFeImage(e.target.value)}
                      placeholder={DEFAULT_SR_FE_IMAGE}
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-sr-cn-image">CN image</Label>
                    <Input
                      id="cpe-sr-cn-image"
                      value={cnImage}
                      onChange={(e) => setCnImage(e.target.value)}
                      placeholder={DEFAULT_SR_CN_IMAGE}
                      required
                    />
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-sr-fe-replicas">FE replicas</Label>
                    <Input
                      id="cpe-sr-fe-replicas"
                      type="number"
                      min={1}
                      value={feReplicas}
                      onChange={(e) => setFeReplicas(Number(e.target.value))}
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-sr-cn-replicas">CN replicas</Label>
                    <Input
                      id="cpe-sr-cn-replicas"
                      type="number"
                      min={1}
                      value={cnReplicas}
                      onChange={(e) => setCnReplicas(Number(e.target.value))}
                      required
                    />
                  </div>
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="cpe-sr-delegation">Per-user delegation</Label>
                  <Select
                    value={delegationMode || "auto"}
                    onValueChange={(v) =>
                      setDelegationMode(
                        v === "auto" ? "" : (v as StarRocksDelegationOption),
                      )
                    }
                  >
                    <SelectTrigger id="cpe-sr-delegation" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="auto">
                        Auto — JWT, else impersonate (recommended)
                      </SelectItem>
                      <SelectItem value="jwt">JWT (per-user)</SelectItem>
                      <SelectItem value="impersonate">
                        Impersonate (service user)
                      </SelectItem>
                      <SelectItem value="none">None (open engine)</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">
                    {DELEGATION_HELP[delegationMode]}
                  </p>
                </div>

                {(delegationMode === "jwt" ||
                  delegationMode === "impersonate") && (
                  <div className="space-y-1.5">
                    <div className="flex items-center justify-between gap-4">
                      <Label htmlFor="cpe-sr-tls" className="font-normal">
                        Enable TLS
                      </Label>
                      <Switch
                        id="cpe-sr-tls"
                        checked={tlsEnabled}
                        onCheckedChange={setTlsEnabled}
                        aria-label="Enable TLS"
                      />
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Enables FE SSL on the MySQL-protocol port. Requires a
                      StarRocks image &ge; 3.4.1 (the default 3.4 line works);
                      an older FE can&apos;t serve the SSL listener. Recommended
                      for both delegation modes.
                    </p>
                  </div>
                )}
              </>
            ) : (
              <>
                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-coordinators">Coordinator replicas</Label>
                    <Input
                      id="cpe-coordinators"
                      type="number"
                      min={1}
                      value={coordinatorReplicas}
                      onChange={(e) => setCoordinatorReplicas(Number(e.target.value))}
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-workers">Worker replicas</Label>
                    <Input
                      id="cpe-workers"
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
                    <Label htmlFor="cpe-cpu-min">CPU min</Label>
                    <Input
                      id="cpe-cpu-min"
                      value={cpuMin}
                      onChange={(e) => setCpuMin(e.target.value)}
                      placeholder="1"
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-cpu-max">CPU max</Label>
                    <Input
                      id="cpe-cpu-max"
                      value={cpuMax}
                      onChange={(e) => setCpuMax(e.target.value)}
                      placeholder="2"
                      required
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cpe-memory">Memory</Label>
                    <Input
                      id="cpe-memory"
                      value={memory}
                      onChange={(e) => setMemory(e.target.value)}
                      placeholder="4Gi"
                      required
                    />
                  </div>
                </div>

                <div className="space-y-1.5">
                  <Label htmlFor="cpe-tls-cert">Coordinator TLS cert</Label>
                  <Select
                    value={tlsCertSecretRef || "__self_signed__"}
                    onValueChange={(v) =>
                      setTlsCertSecretRef(v === "__self_signed__" ? "" : v)
                    }
                  >
                    <SelectTrigger id="cpe-tls-cert" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__self_signed__">
                        Self-signed (default)
                      </SelectItem>
                      {(secrets.data ?? [])
                        .filter((s) => s.type === "kubernetes.io/tls")
                        .map((s) => (
                          <SelectItem key={s.id} value={s.id}>
                            {s.name}
                          </SelectItem>
                        ))}
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">
                    Only used for JWT/TLS engines. Pick a kubernetes.io/tls
                    secret to back the coordinator cert, or keep self-signed.
                  </p>
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
              disabled={
                createMutation.isPending ||
                !name.trim() ||
                !warehouseId ||
                !PROVISIONABLE.has(engineType)
              }
            >
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add engine
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
