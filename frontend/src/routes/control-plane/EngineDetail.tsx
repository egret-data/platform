import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Server, Trash2 } from "lucide-react";

import {
  type CreateEngineInput,
  type Engine,
  type EngineStatus,
  deleteEngine,
  getEngine,
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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

/**
 * Control Plane → Engine detail.
 *
 * Live status (phase + pods, polled every 5s), editable resource config, and a
 * danger-zone delete. Config edits re-provision the cluster via `updateEngine`.
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
  const [error, setError] = useState<string | null>(null);

  // Keep inputs in sync when the polled engine changes (e.g. an external update).
  useEffect(() => {
    setCoordinatorReplicas(engine.spec.coordinatorReplicas);
    setWorkerReplicas(engine.spec.workerReplicas);
    setCpuMin(engine.spec.cpuMin);
    setCpuMax(engine.spec.cpuMax);
    setMemory(engine.spec.memory);
  }, [
    engine.spec.coordinatorReplicas,
    engine.spec.workerReplicas,
    engine.spec.cpuMin,
    engine.spec.cpuMax,
    engine.spec.memory,
  ]);

  const saveMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] }),
  });

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const input: CreateEngineInput = {
      name: engine.name,
      warehouseId: engine.warehouseId,
      coordinatorReplicas,
      workerReplicas,
      cpuMin: cpuMin.trim(),
      cpuMax: cpuMax.trim(),
      memory: memory.trim(),
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
          Adjust cluster size and resources. Saving re-provisions the engine.
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

          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
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
