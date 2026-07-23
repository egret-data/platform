import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2 } from "lucide-react";

import {
  type CreateEngineInput,
  type Engine,
  type Secret,
  type Warehouse,
  createEngine,
  listSecrets,
  listWarehouses,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
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
import {
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
  TRINO_FIELD_DEFAULTS,
  enginesQueryKey,
} from "./ControlPlaneEngines";
import { TrinoConfigEditor } from "./EngineDetail";
import { useEngineConfigForm } from "./useEngineConfigForm";
import type { StarRocksDelegationOption } from "./useStarRocksConfigForm";

// Placeholder image tag shown as the ImageSection default; the backend uses its
// own default when the image field is left blank.
const DEFAULT_TRINO_IMAGE_VERSION = "470";

/**
 * Full-page create-engine wizard (`/control-plane/engines/new`). Mirrors the
 * detail Settings layout: identity header + warehouse, then the SAME left-nav
 * config editor (Trino) so an engine is configured with the full Trino helm
 * values surface at creation time. StarRocks keeps the compact create fields.
 */
export default function EngineCreate() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [sp] = useSearchParams();
  const [engineType, setEngineType] = useState(
    sp.get("type") === "starrocks" ? "starrocks" : "trino",
  );
  const isStarRocks = engineType === "starrocks";

  const warehouses = useQuery<Warehouse[], ApiError>({
    queryKey: ["cp", "warehouses"],
    queryFn: listWarehouses,
    retry: false,
  });
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });
  const createMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: createEngine,
    onSuccess: (engine) => {
      queryClient.invalidateQueries({ queryKey: enginesQueryKey });
      navigate(`/control-plane/engines/${engine.id}`);
    },
  });

  const [name, setName] = useState("");
  const [warehouseId, setWarehouseId] = useState("");
  const [tlsCertSecretRef, setTlsCertSecretRef] = useState("");
  const [rawInvalid, setRawInvalid] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // StarRocks-only compact fields.
  const [feImage, setFeImage] = useState(DEFAULT_SR_FE_IMAGE);
  const [cnImage, setCnImage] = useState(DEFAULT_SR_CN_IMAGE);
  const [feReplicas, setFeReplicas] = useState(1);
  const [cnReplicas, setCnReplicas] = useState(1);
  const [delegationMode, setDelegationMode] =
    useState<StarRocksDelegationOption>("");
  const [tlsEnabled, setTlsEnabled] = useState(true);

  // A synthetic engine seeds the Trino config form in "create mode": the form
  // starts from chart defaults (placeholders) and the save mutation inside the
  // hook is never called — we send `builtTrinoConfig` on create instead.
  const syntheticEngine = useMemo(
    () =>
      ({
        id: "",
        name: "new",
        type: "trino",
        namespace: "",
        warehouseId: "",
        status: "provisioning",
        spec: {
          ...TRINO_FIELD_DEFAULTS,
          imageVersion: DEFAULT_TRINO_IMAGE_VERSION,
          trinoConfig: {},
        },
      }) as unknown as Omit<Engine, "status">,
    [],
  );
  const cfg = useEngineConfigForm(syntheticEngine);

  const provisioned = (warehouses.data ?? []).filter(
    (w) => w.lakekeeperWarehouseId,
  );
  const tlsSecrets = (secrets.data ?? []).filter(
    (s) => s.type === "kubernetes.io/tls",
  );

  const canCreate =
    name.trim().length > 0 &&
    warehouseId.length > 0 &&
    !rawInvalid &&
    !createMutation.isPending;

  async function handleCreate() {
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
            ...(delegationMode !== "" ? { delegationMode, tlsEnabled } : {}),
          },
          ...TRINO_FIELD_DEFAULTS,
        }
      : {
          name: name.trim(),
          warehouseId,
          ...TRINO_FIELD_DEFAULTS,
          trinoConfig: cfg.builtTrinoConfig,
          ...(tlsCertSecretRef ? { tlsCertSecretRef } : {}),
        };
    try {
      await createMutation.mutateAsync(input);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to provision engine.");
    }
  }

  return (
    <div className="space-y-4">
      <Link
        to="/control-plane/engines"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Engines
      </Link>

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <h1 className="text-2xl font-semibold leading-tight">New engine</h1>
          <p className="text-sm text-muted-foreground">
            {isStarRocks
              ? "Provision a StarRocks cluster against a catalog warehouse and set its FE/CN sizing and delegation up front."
              : "Provision a query engine against a catalog warehouse and configure it with the full Trino config surface up front."}
          </p>
        </div>
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {/* Identity: type, name, warehouse. */}
      <section className="space-y-4 rounded-lg border p-4">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <div className="space-y-1.5">
            <Label htmlFor="ec-type">Engine type</Label>
            <Select value={engineType} onValueChange={setEngineType}>
              <SelectTrigger id="ec-type" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="trino">Trino</SelectItem>
                <SelectItem value="starrocks">StarRocks</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ec-name">Name</Label>
            <Input
              id="ec-name"
              value={name}
              placeholder="analytics"
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ec-warehouse">Catalog warehouse</Label>
            <Select value={warehouseId} onValueChange={setWarehouseId}>
              <SelectTrigger id="ec-warehouse" className="w-full">
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
        </div>
      </section>

      {/* Config: full Trino editor, or compact StarRocks fields. */}
      {isStarRocks ? (
        <section className="space-y-4 rounded-lg border p-4">
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="ec-sr-fe-image">FE image</Label>
              <Input
                id="ec-sr-fe-image"
                value={feImage}
                placeholder={DEFAULT_SR_FE_IMAGE}
                onChange={(e) => setFeImage(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ec-sr-cn-image">CN image</Label>
              <Input
                id="ec-sr-cn-image"
                value={cnImage}
                placeholder={DEFAULT_SR_CN_IMAGE}
                onChange={(e) => setCnImage(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ec-sr-fe-replicas">FE replicas</Label>
              <Input
                id="ec-sr-fe-replicas"
                type="number"
                min={1}
                value={feReplicas}
                onChange={(e) => setFeReplicas(Number(e.target.value) || 1)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ec-sr-cn-replicas">CN replicas</Label>
              <Input
                id="ec-sr-cn-replicas"
                type="number"
                min={1}
                value={cnReplicas}
                onChange={(e) => setCnReplicas(Number(e.target.value) || 1)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ec-sr-delegation">Per-user delegation</Label>
              <Select
                value={delegationMode || "auto"}
                onValueChange={(v) =>
                  setDelegationMode(
                    v === "auto" ? "" : (v as StarRocksDelegationOption),
                  )
                }
              >
                <SelectTrigger id="ec-sr-delegation" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="auto">
                    Auto — JWT, else impersonate
                  </SelectItem>
                  <SelectItem value="jwt">JWT (per-user)</SelectItem>
                  <SelectItem value="impersonate">Impersonate</SelectItem>
                  <SelectItem value="none">None (open engine)</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {(delegationMode === "jwt" || delegationMode === "impersonate") && (
              <div className="flex items-center justify-between gap-4 pt-6">
                <Label htmlFor="ec-sr-tls" className="font-normal">
                  Enable TLS
                </Label>
                <Switch
                  id="ec-sr-tls"
                  checked={tlsEnabled}
                  onCheckedChange={setTlsEnabled}
                />
              </div>
            )}
          </div>
        </section>
      ) : (
        <section className="space-y-4">
          <div className="space-y-1">
            <h2 className="text-base font-semibold">Configuration</h2>
            <p className="text-sm text-muted-foreground">
              Cluster-wide, coordinator, and worker Trino config. Leave fields
              blank to use the chart defaults.
            </p>
          </div>
          <TrinoConfigEditor
            cfg={cfg}
            imageVersion={DEFAULT_TRINO_IMAGE_VERSION}
            onInvalidChange={setRawInvalid}
          />
          {tlsSecrets.length > 0 && (
            <div className="max-w-sm space-y-1.5">
              <Label htmlFor="ec-tls-cert">Coordinator TLS cert (optional)</Label>
              <Select
                value={tlsCertSecretRef || "__none__"}
                onValueChange={(v) =>
                  setTlsCertSecretRef(v === "__none__" ? "" : v)
                }
              >
                <SelectTrigger id="ec-tls-cert" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__none__">Self-signed (default)</SelectItem>
                  {tlsSecrets.map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
        </section>
      )}

      <div className="flex justify-end gap-2 border-t pt-4">
        <Button
          variant="outline"
          onClick={() => navigate("/control-plane/engines")}
        >
          Cancel
        </Button>
        <Button onClick={handleCreate} disabled={!canCreate}>
          {createMutation.isPending && (
            <Loader2 className="h-4 w-4 animate-spin" />
          )}
          Create engine
        </Button>
      </div>
    </div>
  );
}
