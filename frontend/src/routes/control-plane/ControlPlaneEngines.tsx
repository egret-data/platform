import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Loader2, Plus, Server } from "lucide-react";

import {
  type CreateEngineInput,
  type Engine,
  type Warehouse,
  createEngine,
  getEngine,
  listEngines,
  listWarehouses,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

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
// falls back to when the body omits them.
const DEFAULT_SR_FE_IMAGE = "starrocks/fe-ubuntu:3.3-latest";
const DEFAULT_SR_CN_IMAGE = "starrocks/cn-ubuntu:3.3-latest";

// Harmless defaults for the still-required Trino-only body fields
// (coordinatorReplicas/workerReplicas/cpuMin/cpuMax/memory) when creating a
// StarRocks engine — the backend ignores them for `kind: "starrocks"` but the
// body validates them as required regardless of kind. Mirrors the Trino
// form's own defaults below.
const TRINO_FIELD_DEFAULTS = {
  coordinatorReplicas: 1,
  workerReplicas: 2,
  cpuMin: "1",
  cpuMax: "2",
  memory: "4Gi",
};

const COLUMNS = ["Name", "Type", "Endpoint", "Status", "Actions"];

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

  return (
    <Card data-testid="engines-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Server className="h-5 w-5 text-muted-foreground" />
              Engines
            </CardTitle>
            <CardDescription>
              Platform query engines. Provision a Trino or StarRocks cluster
              against a catalog warehouse and monitor its health. RisingWave
              arrives in a later phase.
            </CardDescription>
          </div>
          <Button
            onClick={() => setAddOpen(true)}
            disabled={!canProvision}
            title={canProvision ? undefined : "Coming soon"}
          >
            <Plus className="h-4 w-4" />
            Add engine
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {(() => {
          const all = engines.data ?? [];
          const total = all.length;
          const running = all.filter((e) => (e.status ?? "").toLowerCase() === "running").length;
          const down = total - running;
          return (
            <SummaryStrip
              items={[
                { label: "engines", value: total },
                { label: "running", value: running },
                { label: "not running", value: down },
              ]}
            />
          );
        })()}
        {engines.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load engines. Please try again.
            </AlertDescription>
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
              <TabsContent key={type.value} value={type.value}>
                <EngineTable
                  label={type.label}
                  rows={(engines.data ?? []).filter((e) => e.type === type.value)}
                  loading={engines.isLoading}
                />
              </TabsContent>
            ))}
          </Tabs>
        )}
      </CardContent>

      <AddEngineDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        engineType={activeType}
        engineLabel={activeLabel}
      />
    </Card>
  );
}

function EngineTable({
  label,
  rows,
  loading,
}: {
  label: string;
  rows: Engine[];
  loading: boolean;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          {COLUMNS.map((col) => (
            <TableHead
              key={col}
              className={col === "Actions" ? "text-right" : undefined}
            >
              {col === "Actions" ? <span className="sr-only">Actions</span> : col}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.length === 0 ? (
          <TableRow className="hover:bg-transparent">
            <TableCell
              colSpan={COLUMNS.length}
              className="py-10 text-center text-muted-foreground"
            >
              {loading ? "Loading…" : `No ${label} engines yet.`}
            </TableCell>
          </TableRow>
        ) : (
          rows.map((engine) => (
            <TableRow key={engine.id}>
              <TableCell className="font-medium">
                <Link
                  to={`/control-plane/engines/${engine.id}`}
                  className="text-primary hover:underline"
                >
                  {engine.name}
                </Link>
              </TableCell>
              <TableCell>
                <Badge variant="secondary">{engine.type}</Badge>
              </TableCell>
              <TableCell className="font-mono text-xs text-muted-foreground">
                {engine.name}
                {engine.type === "starrocks" ? "-fe-service" : "-coordinator"}.
                {engine.namespace}
              </TableCell>
              <TableCell>
                <EngineStatusBadge id={engine.id} fallback={engine.status} />
              </TableCell>
              <TableCell className="text-right">
                <Button variant="ghost" size="sm" asChild>
                  <Link
                    to={`/control-plane/engines/${engine.id}`}
                    aria-label={`View ${engine.name}`}
                  >
                    <ExternalLink className="h-4 w-4" />
                    Details
                  </Link>
                </Button>
              </TableCell>
            </TableRow>
          ))
        )}
      </TableBody>
    </Table>
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
