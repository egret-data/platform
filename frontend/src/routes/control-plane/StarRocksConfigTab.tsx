import { useSearchParams } from "react-router-dom";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SaveBar } from "./EngineDetail";
import {
  type RoleResourcesForm,
  type StarRocksConfigForm,
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
} from "./useStarRocksConfigForm";

// Left-nav sections for the StarRocks Configuration tab. Storage is read-only
// (configured from the warehouse at creation), the rest are editable.
const NAV_ITEMS: { key: string; label: string }[] = [
  { key: "image", label: "Image" },
  { key: "compute", label: "Compute" },
  { key: "resources", label: "Resources" },
  { key: "storage", label: "Storage" },
];

/**
 * Control Plane → Engine detail → Configuration (StarRocks).
 *
 * StarRocks-shaped mirror of the Trino `ConfigurationTab` shell: a `Card` with a
 * left-nav of sections + a content pane + the shared `SaveBar`. Edits FE/CN
 * image, replicas, and per-role resources; storage is read-only. Takes the
 * `useStarRocksConfigForm` instance from `EngineTabs` so a future Security tab
 * can share the same form state (and Save).
 */
export function StarRocksConfigTab({ cfg }: { cfg: StarRocksConfigForm }) {
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

  return (
    <Card>
      <CardHeader>
        <div className="space-y-1.5">
          <CardTitle className="text-lg">Configuration</CardTitle>
          <CardDescription>
            FE/CN image, replicas, and compute resources. Saving re-provisions
            the engine.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <div className="flex flex-col gap-4 md:flex-row">
            <nav className="shrink-0 space-y-1 md:w-48">
              {NAV_ITEMS.map((item) => {
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
                  </button>
                );
              })}
            </nav>

            <div className="min-w-0 flex-1">
              {section === "image" && <ImageSection cfg={cfg} />}
              {section === "compute" && <ComputeSection cfg={cfg} />}
              {section === "resources" && <ResourcesSection cfg={cfg} />}
              {section === "storage" && <StorageSection cfg={cfg} />}
            </div>
          </div>

          <SaveBar cfg={cfg} />
        </div>
      </CardContent>
    </Card>
  );
}

function ImageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <div className="space-y-1.5">
        <Label htmlFor="sr-fe-image">FE image</Label>
        <Input
          id="sr-fe-image"
          value={cfg.feImage}
          onChange={(e) => cfg.setFeImage(e.target.value)}
          placeholder={DEFAULT_SR_FE_IMAGE}
        />
        <p className="text-xs text-muted-foreground">
          Frontend (metadata) container image.
        </p>
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="sr-cn-image">CN image</Label>
        <Input
          id="sr-cn-image"
          value={cfg.cnImage}
          onChange={(e) => cfg.setCnImage(e.target.value)}
          placeholder={DEFAULT_SR_CN_IMAGE}
        />
        <p className="text-xs text-muted-foreground">
          Compute node container image.
        </p>
      </div>
    </div>
  );
}

function ComputeSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <div className="space-y-1.5">
        <Label htmlFor="sr-fe-replicas">FE replicas</Label>
        <Input
          id="sr-fe-replicas"
          type="number"
          min={1}
          value={cfg.feReplicas}
          onChange={(e) => cfg.setFeReplicas(e.target.value)}
        />
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="sr-cn-replicas">CN replicas</Label>
        <Input
          id="sr-cn-replicas"
          type="number"
          min={1}
          value={cfg.cnReplicas}
          onChange={(e) => cfg.setCnReplicas(e.target.value)}
        />
      </div>
    </div>
  );
}

/** One role's CPU/memory request+limit inputs. */
function ResourceFields({
  role,
  prefix,
  value,
  onChange,
}: {
  role: string;
  prefix: string;
  value: RoleResourcesForm;
  onChange: (v: RoleResourcesForm) => void;
}) {
  const field = (
    key: keyof RoleResourcesForm,
    label: string,
    placeholder: string,
  ) => (
    <div className="space-y-1.5">
      <Label htmlFor={`${prefix}-${key}`}>{`${role} ${label}`}</Label>
      <Input
        id={`${prefix}-${key}`}
        value={value[key]}
        onChange={(e) => onChange({ ...value, [key]: e.target.value })}
        placeholder={placeholder}
      />
    </div>
  );
  return (
    <div className="space-y-3">
      <p className="text-sm font-medium">{role}</p>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        {field("cpuRequest", "CPU request", "1")}
        {field("cpuLimit", "CPU limit", "2")}
        {field("memRequest", "memory request", "4Gi")}
        {field("memLimit", "memory limit", "8Gi")}
      </div>
    </div>
  );
}

function ResourcesSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="space-y-6">
      <ResourceFields
        role="FE"
        prefix="sr-fe-res"
        value={cfg.feResources}
        onChange={cfg.setFeResources}
      />
      <ResourceFields
        role="CN"
        prefix="sr-cn-res"
        value={cfg.cnResources}
        onChange={cfg.setCnResources}
      />
      <p className="text-xs text-muted-foreground">
        Leave a field blank to use the default sizing. Kubernetes resource
        strings (e.g. <code className="font-mono">500m</code>,{" "}
        <code className="font-mono">4Gi</code>).
      </p>
    </div>
  );
}

function StorageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const endpoint = cfg.storedConfig?.storageEndpoint;
  const path = cfg.storedConfig?.storagePath;
  if (endpoint || path) {
    return (
      <dl className="space-y-3 text-sm">
        {endpoint && (
          <div className="space-y-0.5">
            <dt className="text-muted-foreground">Endpoint</dt>
            <dd className="font-mono text-xs">{endpoint}</dd>
          </div>
        )}
        {path && (
          <div className="space-y-0.5">
            <dt className="text-muted-foreground">Path</dt>
            <dd className="font-mono text-xs">{path}</dd>
          </div>
        )}
        <p className="text-xs text-muted-foreground">
          Object storage is configured from the warehouse at creation and
          isn&apos;t editable here.
        </p>
      </dl>
    );
  }
  return (
    <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
      Object storage is configured from the warehouse at creation and isn&apos;t
      editable here.
    </p>
  );
}
