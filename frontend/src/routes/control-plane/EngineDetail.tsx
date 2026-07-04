import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ChevronDown, Loader2, Plus, Server, Trash2 } from "lucide-react";

import {
  type AddCatalogInput,
  type Engine,
  type EngineCatalog,
  type EngineStatus,
  type Secret,
  type Storage,
  type Warehouse,
  addEngineCatalog,
  deleteEngine,
  deleteEngineCatalog,
  getEngine,
  listEngineCatalogs,
  listSecrets,
  listStorage,
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
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
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
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  type Kv,
  KeyValueEditor,
  ResourceGroupsSection,
  SessionPropertiesSection,
  kvToRecord,
} from "./EngineResourceGroups";
import {
  type ProbeForm,
  type RoleForm,
  PROBE_FIELDS,
  useEngineConfigForm,
} from "./useEngineConfigForm";
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
        <p className="py-14 text-center text-sm text-muted-foreground">Loading…</p>
      ) : (
        <Tabs defaultValue="overview" className="space-y-5">
          <TabsList>
            <TabsTrigger value="overview">Overview</TabsTrigger>
            <TabsTrigger value="monitor">Monitor</TabsTrigger>
          </TabsList>
          <TabsContent value="overview" className="space-y-5">
            <StatusCard status={data.status} />
            <ConfigCard engine={data} />
            <CatalogsCard engine={data} />
            <DangerCard engine={data} />
          </TabsContent>
          <TabsContent value="monitor">
            <EngineMonitor engineId={id} />
          </TabsContent>
        </Tabs>
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

const LOG_LEVELS = ["INFO", "DEBUG", "WARN", "ERROR"] as const;

// Per-role config form model + serialization lives in `useEngineConfigForm.ts`;
// `RoleForm`, `ProbeForm`, and `PROBE_FIELDS` are imported from there.

/** A list of free-text lines (used for `additionalJvmConfig`). */
function StringListEditor({
  rows,
  onChange,
  placeholder,
}: {
  rows: string[];
  onChange: (rows: string[]) => void;
  placeholder?: string;
}) {
  return (
    <div className="space-y-2">
      {rows.map((row, i) => (
        <div key={i} className="flex items-center gap-2">
          <Input
            value={row}
            placeholder={placeholder}
            onChange={(e) =>
              onChange(rows.map((r, j) => (j === i ? e.target.value : r)))
            }
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
            aria-label="Remove line"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, ""])}
      >
        <Plus className="h-4 w-4" />
        Add JVM flag
      </Button>
    </div>
  );
}

/** A single collapsible probe editor (liveness/readiness/startup). */
function ProbeEditor({
  label,
  idPrefix,
  value,
  onChange,
}: {
  label: string;
  idPrefix: string;
  value: ProbeForm;
  onChange: (next: ProbeForm) => void;
}) {
  return (
    <Collapsible className="rounded-md border">
      <CollapsibleTrigger className="group flex w-full items-center justify-between px-3 py-2 text-sm font-medium">
        {label} probe
        <ChevronDown className="h-4 w-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
      </CollapsibleTrigger>
      <CollapsibleContent className="border-t px-3 py-3">
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
          {PROBE_FIELDS.map((f) => (
            <div key={f.key} className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-${f.key}`} className="text-xs">
                {f.label}
              </Label>
              <Input
                id={`${idPrefix}-${f.key}`}
                type="number"
                min={0}
                value={value[f.key]}
                placeholder="default"
                onChange={(e) => onChange({ ...value, [f.key]: e.target.value })}
              />
            </div>
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Shared editor for a Trino role (coordinator or worker) config block. */
function RoleConfigEditor({
  value,
  onChange,
  idPrefix,
}: {
  value: RoleForm;
  onChange: (next: RoleForm) => void;
  idPrefix: string;
}) {
  const set = <K extends keyof RoleForm>(key: K, v: RoleForm[K]) =>
    onChange({ ...value, [key]: v });

  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-replicas`}>Replicas</Label>
          <Input
            id={`${idPrefix}-replicas`}
            type="number"
            min={0}
            value={value.replicas}
            placeholder="1"
            onChange={(e) => set("replicas", e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-2">
        <Label>Resources</Label>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cpu-req`} className="text-xs">
              CPU request
            </Label>
            <Input
              id={`${idPrefix}-cpu-req`}
              value={value.cpuRequest}
              placeholder="1"
              onChange={(e) => set("cpuRequest", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cpu-lim`} className="text-xs">
              CPU limit
            </Label>
            <Input
              id={`${idPrefix}-cpu-lim`}
              value={value.cpuLimit}
              placeholder="2"
              onChange={(e) => set("cpuLimit", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-mem-req`} className="text-xs">
              Memory request
            </Label>
            <Input
              id={`${idPrefix}-mem-req`}
              value={value.memRequest}
              placeholder="2Gi"
              onChange={(e) => set("memRequest", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-mem-lim`} className="text-xs">
              Memory limit
            </Label>
            <Input
              id={`${idPrefix}-mem-lim`}
              value={value.memLimit}
              placeholder="4Gi"
              onChange={(e) => set("memLimit", e.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="space-y-2">
        <Label>JVM heap</Label>
        <RadioGroup
          className="flex gap-6"
          value={value.heapMode}
          onValueChange={(v) => set("heapMode", v as RoleForm["heapMode"])}
        >
          <div className="flex items-center gap-2">
            <RadioGroupItem value="fixed" id={`${idPrefix}-heap-fixed`} />
            <Label htmlFor={`${idPrefix}-heap-fixed`} className="font-normal">
              Fixed size
            </Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem value="percent" id={`${idPrefix}-heap-percent`} />
            <Label htmlFor={`${idPrefix}-heap-percent`} className="font-normal">
              Percent
            </Label>
          </div>
        </RadioGroup>
        {value.heapMode === "fixed" ? (
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-max`} className="text-xs">
                Max size (-Xmx)
              </Label>
              <Input
                id={`${idPrefix}-heap-max`}
                value={value.maxSize}
                placeholder="2G"
                onChange={(e) => set("maxSize", e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-min`} className="text-xs">
                Min size (-Xms)
              </Label>
              <Input
                id={`${idPrefix}-heap-min`}
                value={value.minSize}
                placeholder="2G"
                onChange={(e) => set("minSize", e.target.value)}
              />
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-maxpct`} className="text-xs">
                Max RAM percent
              </Label>
              <Input
                id={`${idPrefix}-heap-maxpct`}
                type="number"
                min={0}
                max={100}
                value={value.maxPercent}
                placeholder="80"
                onChange={(e) => set("maxPercent", e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-initpct`} className="text-xs">
                Initial RAM percent
              </Label>
              <Input
                id={`${idPrefix}-heap-initpct`}
                type="number"
                min={0}
                max={100}
                value={value.initialPercent}
                placeholder="80"
                onChange={(e) => set("initialPercent", e.target.value)}
              />
            </div>
          </div>
        )}
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-qmmpn`}>Query max memory / node</Label>
          <Input
            id={`${idPrefix}-qmmpn`}
            value={value.queryMaxMemoryPerNode}
            placeholder="1GB"
            onChange={(e) => set("queryMaxMemoryPerNode", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-hhpn`}>Heap headroom / node</Label>
          <Input
            id={`${idPrefix}-hhpn`}
            value={value.heapHeadroomPerNode}
            placeholder="2GB"
            onChange={(e) => set("heapHeadroomPerNode", e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-1.5">
        <Label>Additional JVM config</Label>
        <p className="text-xs text-muted-foreground">
          Extra lines appended to <code>jvm.config</code>.
        </p>
        <StringListEditor
          rows={value.additionalJvmConfig}
          onChange={(rows) => set("additionalJvmConfig", rows)}
          placeholder="-XX:MaxHeapFreeRatio=100"
        />
      </div>

      <div className="space-y-2">
        <Label>Probes</Label>
        <div className="space-y-2">
          <ProbeEditor
            label="Liveness"
            idPrefix={`${idPrefix}-live`}
            value={value.liveness}
            onChange={(p) => set("liveness", p)}
          />
          <ProbeEditor
            label="Readiness"
            idPrefix={`${idPrefix}-ready`}
            value={value.readiness}
            onChange={(p) => set("readiness", p)}
          />
          <ProbeEditor
            label="Startup"
            idPrefix={`${idPrefix}-startup`}
            value={value.startup}
            onChange={(p) => set("startup", p)}
          />
        </div>
      </div>
    </div>
  );
}

function ConfigCard({ engine }: { engine: EngineWithStatus }) {
  const storage = useQuery<Storage[], ApiError>({
    queryKey: ["cp", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });

  // All config form state, prefill, dirty tracking, and the save mutation +
  // payload builder live in the shared hook. Destructure to the same names the
  // JSX below already uses.
  const cfg = useEngineConfigForm(engine);
  const {
    queryMaxMemory,
    setQueryMaxMemory,
    logLevel,
    setLogLevel,
    configProps,
    setConfigProps,
    env,
    setEnv,
    eventListener,
    setEventListener,
    nodeProps,
    setNodeProps,
    logProps,
    setLogProps,
    ftEnabled,
    setFtEnabled,
    retryPolicy,
    setRetryPolicy,
    exchangeStorageConnId,
    setExchangeStorageConnId,
    exchangePrefix,
    setExchangePrefix,
    coordinator,
    setCoordinator,
    worker,
    setWorker,
    gsEnabled,
    setGsEnabled,
    gracePeriod,
    setGracePeriod,
    resourceGroups,
    setResourceGroups,
    sessionRules,
    setSessionRules,
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
    error,
    dirty,
    saving,
    saved,
    save,
  } = cfg;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Configuration</CardTitle>
        <CardDescription>
          Cluster-wide, coordinator, and worker Trino config. Saving re-provisions
          the engine.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <Tabs defaultValue="cluster">
            <TabsList>
              <TabsTrigger value="cluster">Cluster</TabsTrigger>
              <TabsTrigger value="coordinator">Coordinator</TabsTrigger>
              <TabsTrigger value="worker">Worker</TabsTrigger>
            </TabsList>

            <TabsContent value="cluster" className="space-y-5 pt-4">
              <div className="grid grid-cols-2 gap-4">
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
                  <Label htmlFor="ced-log-level">Log level</Label>
                  <Select value={logLevel} onValueChange={setLogLevel}>
                    <SelectTrigger id="ced-log-level" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {LOG_LEVELS.map((lvl) => (
                        <SelectItem key={lvl} value={lvl}>
                          {lvl}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <div className="space-y-1.5">
                <Label>Config properties</Label>
                <p className="text-xs text-muted-foreground">
                  Raw <code>config.properties</code> key/value pairs (both roles).
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

              <div className="space-y-3 rounded-lg border p-4">
                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="ced-ft-enabled">
                      Fault-tolerant execution
                    </Label>
                    <p className="text-xs text-muted-foreground">
                      Retry failed tasks/queries, spooling exchange data to an S3
                      storage connection.
                    </p>
                  </div>
                  <Switch
                    id="ced-ft-enabled"
                    checked={ftEnabled}
                    onCheckedChange={setFtEnabled}
                  />
                </div>
                {ftEnabled && (
                  <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
                    <div className="space-y-1.5">
                      <Label htmlFor="ced-ft-retry">Retry policy</Label>
                      <Select
                        value={retryPolicy}
                        onValueChange={(v) =>
                          setRetryPolicy(v as "TASK" | "QUERY")
                        }
                      >
                        <SelectTrigger id="ced-ft-retry" className="w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="TASK">TASK</SelectItem>
                          <SelectItem value="QUERY">QUERY</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="ced-ft-storage">Exchange storage</Label>
                      <Select
                        value={exchangeStorageConnId}
                        onValueChange={setExchangeStorageConnId}
                      >
                        <SelectTrigger id="ced-ft-storage" className="w-full">
                          <SelectValue placeholder="Select a connection" />
                        </SelectTrigger>
                        <SelectContent>
                          {(storage.data ?? []).length === 0 ? (
                            <SelectItem value="__none__" disabled>
                              No storage connections
                            </SelectItem>
                          ) : (
                            (storage.data ?? []).map((s) => (
                              <SelectItem key={s.id} value={s.id}>
                                {s.name}
                              </SelectItem>
                            ))
                          )}
                        </SelectContent>
                      </Select>
                    </div>
                    <div className="space-y-1.5">
                      <Label htmlFor="ced-ft-prefix">Exchange prefix</Label>
                      <Input
                        id="ced-ft-prefix"
                        value={exchangePrefix}
                        placeholder="exchange"
                        onChange={(e) => setExchangePrefix(e.target.value)}
                      />
                    </div>
                  </div>
                )}
              </div>

              <div className="space-y-1.5">
                <Label>Event listener</Label>
                <p className="text-xs text-muted-foreground">
                  Raw <code>event-listener.properties</code> key/value pairs.
                </p>
                <KeyValueEditor
                  rows={eventListener}
                  onChange={setEventListener}
                  keyPlaceholder="event-listener.name"
                  valuePlaceholder="http"
                />
              </div>

              <div className="space-y-1.5">
                <Label>Additional node properties</Label>
                <p className="text-xs text-muted-foreground">
                  Extra <code>node.properties</code> key/value pairs.
                </p>
                <KeyValueEditor
                  rows={nodeProps}
                  onChange={setNodeProps}
                  keyPlaceholder="node.internal-address-source"
                  valuePlaceholder="FQDN"
                />
              </div>

              <div className="space-y-1.5">
                <Label>Additional log properties</Label>
                <p className="text-xs text-muted-foreground">
                  Extra <code>log.properties</code> logger levels.
                </p>
                <KeyValueEditor
                  rows={logProps}
                  onChange={setLogProps}
                  keyPlaceholder="io.trino.execution"
                  valuePlaceholder="DEBUG"
                />
              </div>

              <div className="rounded-lg border p-4">
                <ResourceGroupsSection
                  value={resourceGroups}
                  onChange={setResourceGroups}
                />
              </div>

              <div className="rounded-lg border p-4">
                <SessionPropertiesSection
                  value={sessionRules}
                  onChange={setSessionRules}
                />
              </div>

              <div className="space-y-5 rounded-lg border p-4">
                <div className="space-y-0.5">
                  <Label className="text-base">Exposure &amp; auth</Label>
                  <p className="text-xs text-muted-foreground">
                    Authenticate clients and expose the coordinator outside the
                    cluster.
                  </p>
                </div>

                {/* Password users */}
                <div className="space-y-2">
                  <Label>Password users</Label>
                  <p className="text-xs text-muted-foreground">
                    Each user signs in with a username and the password held in
                    the selected egret secret (hashed server-side).
                  </p>
                  <div className="space-y-2">
                    {passwordUsers.map((row, i) => (
                      <div key={i} className="flex items-center gap-2">
                        <Input
                          value={row.username}
                          placeholder="username"
                          aria-label="Username"
                          onChange={(e) =>
                            setPasswordUsers(
                              passwordUsers.map((r, j) =>
                                j === i ? { ...r, username: e.target.value } : r,
                              ),
                            )
                          }
                        />
                        <Select
                          value={row.secretRef}
                          onValueChange={(v) =>
                            setPasswordUsers(
                              passwordUsers.map((r, j) =>
                                j === i ? { ...r, secretRef: v } : r,
                              ),
                            )
                          }
                        >
                          <SelectTrigger className="w-full" aria-label="Password secret">
                            <SelectValue placeholder="Password secret" />
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
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="shrink-0 text-muted-foreground"
                          onClick={() =>
                            setPasswordUsers(
                              passwordUsers.filter((_, j) => j !== i),
                            )
                          }
                          aria-label="Remove user"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    ))}
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        setPasswordUsers([
                          ...passwordUsers,
                          { username: "", secretRef: "" },
                        ])
                      }
                    >
                      <Plus className="h-4 w-4" />
                      Add user
                    </Button>
                  </div>
                </div>

                {/* JWT */}
                <div className="space-y-3">
                  <div className="flex items-center justify-between gap-4">
                    <div className="space-y-0.5">
                      <Label htmlFor="ced-jwt-enabled">Accept egret JWT</Label>
                      <p className="text-xs text-muted-foreground">
                        Also accept egret-issued JWTs (e.g. from notebooks) as
                        client credentials.
                      </p>
                    </div>
                    <Switch
                      id="ced-jwt-enabled"
                      checked={jwtEnabled}
                      onCheckedChange={setJwtEnabled}
                    />
                  </div>
                  {jwtEnabled && (
                    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                      <div className="space-y-1.5">
                        <Label htmlFor="ced-jwt-jwks">JWKS URI</Label>
                        <Input
                          id="ced-jwt-jwks"
                          value={jwksUri}
                          placeholder="default IdP JWKS"
                          onChange={(e) => setJwksUri(e.target.value)}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label htmlFor="ced-jwt-aud">Required audience</Label>
                        <Input
                          id="ced-jwt-aud"
                          value={requiredAudience}
                          placeholder="optional"
                          onChange={(e) => setRequiredAudience(e.target.value)}
                        />
                      </div>
                    </div>
                  )}
                </div>

                {/* Ingress */}
                <div className="space-y-3">
                  <div className="flex items-center justify-between gap-4">
                    <div className="space-y-0.5">
                      <Label htmlFor="ced-ing-enabled">Expose via ingress</Label>
                      <p className="text-xs text-muted-foreground">
                        Create a Kubernetes Ingress for the coordinator. TLS
                        terminates at the ingress.
                      </p>
                    </div>
                    <Switch
                      id="ced-ing-enabled"
                      checked={ingressEnabled}
                      onCheckedChange={setIngressEnabled}
                    />
                  </div>
                  {ingressEnabled && (
                    <div className="space-y-4">
                      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                        <div className="space-y-1.5">
                          <Label htmlFor="ced-ing-host">Host</Label>
                          <Input
                            id="ced-ing-host"
                            value={ingressHost}
                            placeholder="trino-demo.example.com"
                            onChange={(e) => setIngressHost(e.target.value)}
                          />
                        </div>
                        <div className="space-y-1.5">
                          <Label htmlFor="ced-ing-class">Ingress class</Label>
                          <Input
                            id="ced-ing-class"
                            value={ingressClassName}
                            placeholder="nginx"
                            onChange={(e) => setIngressClassName(e.target.value)}
                          />
                        </div>
                      </div>
                      <div className="space-y-1.5">
                        <Label htmlFor="ced-ing-tls">TLS secret</Label>
                        <Input
                          id="ced-ing-tls"
                          value={ingressTlsSecret}
                          placeholder="optional — Kubernetes TLS secret name"
                          onChange={(e) => setIngressTlsSecret(e.target.value)}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label>Annotations</Label>
                        <p className="text-xs text-muted-foreground">
                          Extra ingress annotations (e.g. controller tuning).
                        </p>
                        <KeyValueEditor
                          rows={ingressAnnotations}
                          onChange={setIngressAnnotations}
                          keyPlaceholder="nginx.ingress.kubernetes.io/proxy-read-timeout"
                          valuePlaceholder="300"
                          addLabel="Add annotation"
                        />
                      </div>
                      {ingressHost.trim() && (
                        <p className="text-xs text-muted-foreground">
                          Clients connect to{" "}
                          <code>https://{ingressHost.trim()}</code> with their
                          username + password (or an egret JWT).
                        </p>
                      )}
                    </div>
                  )}
                </div>
              </div>
            </TabsContent>

            <TabsContent value="coordinator" className="pt-4">
              <RoleConfigEditor
                idPrefix="ced-coord"
                value={coordinator}
                onChange={setCoordinator}
              />
            </TabsContent>

            <TabsContent value="worker" className="space-y-5 pt-4">
              <RoleConfigEditor
                idPrefix="ced-worker"
                value={worker}
                onChange={setWorker}
              />
              <div className="space-y-3 rounded-lg border p-4">
                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="ced-gs-enabled">Graceful shutdown</Label>
                    <p className="text-xs text-muted-foreground">
                      Drain in-flight queries before a worker terminates.
                    </p>
                  </div>
                  <Switch
                    id="ced-gs-enabled"
                    checked={gsEnabled}
                    onCheckedChange={setGsEnabled}
                  />
                </div>
                {gsEnabled && (
                  <div className="space-y-1.5">
                    <Label htmlFor="ced-gs-grace">Grace period (s)</Label>
                    <Input
                      id="ced-gs-grace"
                      type="number"
                      min={0}
                      value={gracePeriod}
                      placeholder="120"
                      onChange={(e) => setGracePeriod(e.target.value)}
                    />
                  </div>
                )}
              </div>
            </TabsContent>
          </Tabs>

          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {saved && !error && (
            <p className="rounded-md border border-primary/30 bg-primary/5 px-3 py-2 text-sm text-muted-foreground">
              Saved. The engine will restart to apply the new configuration.
            </p>
          )}

          <div className="flex justify-end">
            <Button type="button" onClick={save} disabled={!dirty || saving}>
              {saving && <Loader2 className="h-4 w-4 animate-spin" />}
              Save changes
            </Button>
          </div>
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
