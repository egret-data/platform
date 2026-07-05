import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";

import {
  type AddCatalogInput,
  type EngineCatalog,
  type EngineCatalogDetail,
  type Secret,
  type Warehouse,
  addEngineCatalog,
  listSecrets,
  listWarehouses,
  updateEngineCatalog,
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
import { type Kv, kvToRecord, recordToKv } from "./EngineResourceGroups";
import { CATALOG_KINDS } from "./config/catalogKinds";
import { connectorByName, connectors } from "./config/connectorCatalog";
import { KeyedPropertyList } from "./config/KeyedPropertyList";

const DEFAULT_SECRET_PROPERTY = "connection-password";

export interface CatalogFormProps {
  mode: "add" | "edit";
  engineId: string;
  /** Edit only: the fetched detail to prefill from (kind + redacted config + secretRef). */
  initial?: EngineCatalogDetail;
  /** Called after a successful add/update — the parent should invalidate its catalog queries. */
  onSubmitted: () => void;
}

/**
 * The catalog add/edit form: fields vary per `kind` (egret-warehouse warehouse
 * picker / postgresql host+port+db+user+secret / generic connector +
 * properties + optional secret), plus a submit button. Extracted from the
 * former `AddCatalogDialog` (EngineDetail.tsx) so the same fields can back
 * both the "Add catalog" flow and editing an existing catalog.
 *
 * In `edit` mode `name`/`kind` are immutable (rendered as static text) and an
 * `egret-warehouse` catalog is entirely read-only — its config is
 * system-managed (minted client credential) and the backend rejects edits to
 * it. Callers should mount a fresh instance (e.g. via a `key`) per catalog
 * being edited rather than reusing one instance across catalogs.
 */
export function CatalogForm({
  mode,
  engineId,
  initial,
  onSubmitted,
}: CatalogFormProps) {
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

  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState(initial?.kind ?? "egret-warehouse");
  const [error, setError] = useState<string | null>(null);

  // egret-warehouse (add only — edit is read-only, config is system-managed)
  const [warehouseId, setWarehouseId] = useState("");

  // postgresql
  const [host, setHost] = useState(() => String(initial?.config?.host ?? ""));
  const [port, setPort] = useState(() =>
    String(initial?.config?.port ?? "5432"),
  );
  const [database, setDatabase] = useState(() =>
    String(initial?.config?.database ?? ""),
  );
  const [user, setUser] = useState(() => String(initial?.config?.user ?? ""));
  const [secretRef, setSecretRef] = useState(initial?.secretRef ?? "");

  // generic
  const initialConnectorName = String(initial?.config?.connectorName ?? "");
  const [connectorName, setConnectorName] = useState(initialConnectorName);
  // True once the user explicitly picks "Other (custom)…" (or, in edit mode,
  // the prefilled connector isn't in the curated catalog), so the empty
  // initial state shows the placeholder rather than pre-selecting "custom".
  const [customConnector, setCustomConnector] = useState<boolean>(
    () => initialConnectorName !== "" && !connectorByName[initialConnectorName],
  );
  const [genericProps, setGenericProps] = useState<Kv[]>(() =>
    recordToKv(initial?.config?.properties as Record<string, string> | undefined),
  );
  const [secretProperty, setSecretProperty] = useState(() =>
    String(initial?.config?.secretProperty ?? DEFAULT_SECRET_PROPERTY),
  );

  const addMutation = useMutation<EngineCatalog, ApiError, AddCatalogInput>({
    mutationFn: (input) => addEngineCatalog(engineId, input),
  });
  const updateMutation = useMutation<
    EngineCatalog,
    ApiError,
    { config: Record<string, unknown>; secretRef?: string | null }
  >({
    mutationFn: (input) => updateEngineCatalog(engineId, initial!.id, input),
  });
  const pending =
    mode === "add" ? addMutation.isPending : updateMutation.isPending;

  // Only provisioned warehouses (embedded catalog id resolved) can back a catalog.
  const provisioned = (warehouses.data ?? []).filter(
    (w) => w.lakekeeperWarehouseId,
  );

  const readOnlyWarehouse = mode === "edit" && kind === "egret-warehouse";

  function isValid(): boolean {
    if (mode === "add" && !name.trim()) return false;
    switch (kind) {
      case "egret-warehouse":
        return mode === "add" ? warehouseId !== "" : false;
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

  function resetAfterAdd() {
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
    setSecretProperty(DEFAULT_SECRET_PROPERTY);
    addMutation.reset();
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    let config: Record<string, unknown> = {};
    let ref: string | null = null;
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
        ref = secretRef || null;
        break;
      case "generic":
        config = {
          connectorName: connectorName.trim(),
          properties: kvToRecord(genericProps),
          secretProperty: secretProperty.trim() || DEFAULT_SECRET_PROPERTY,
        };
        ref = secretRef || null;
        break;
      default:
        config = {};
    }

    try {
      if (mode === "add") {
        const input: AddCatalogInput = {
          name: name.trim(),
          kind,
          config,
          secretRef: ref,
        };
        await addMutation.mutateAsync(input);
        resetAfterAdd();
      } else {
        await updateMutation.mutateAsync({ config, secretRef: ref });
      }
      onSubmitted();
    } catch (err) {
      setError(
        (err as ApiError).message ??
          `Failed to ${mode === "add" ? "add" : "update"} catalog.`,
      );
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="space-y-1.5">
        <Label htmlFor="cat-name">Name</Label>
        {mode === "add" ? (
          <Input
            id="cat-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. warehouse2"
            required
            autoFocus
          />
        ) : (
          <p id="cat-name" className="text-sm font-medium">
            {name}
          </p>
        )}
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="cat-kind">Kind</Label>
        {mode === "add" ? (
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
        ) : (
          <p id="cat-kind" className="text-sm text-muted-foreground">
            {CATALOG_KINDS.find((k) => k.value === kind)?.label ?? kind}
          </p>
        )}
      </div>

      {kind === "egret-warehouse" &&
        (readOnlyWarehouse ? (
          <div className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
            <p>
              Warehouse:{" "}
              <span className="font-medium text-foreground">
                {String(initial?.config?.warehouse ?? "—")}
              </span>
            </p>
            <p className="mt-1">
              This catalog&rsquo;s configuration is system-managed and cannot
              be edited.
            </p>
          </div>
        ) : (
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
        ))}

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
          <div className="space-y-1.5">
            <Label htmlFor="cat-generic-secret">Secret (optional)</Label>
            <Select
              value={secretRef || "__none__"}
              onValueChange={(v) => setSecretRef(v === "__none__" ? "" : v)}
            >
              <SelectTrigger id="cat-generic-secret" className="w-full">
                <SelectValue placeholder="No secret" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__none__">No secret</SelectItem>
                {(secrets.data ?? []).map((s) => (
                  <SelectItem key={s.id} value={s.id}>
                    {s.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="cat-generic-secret-prop">Secret property</Label>
            <Input
              id="cat-generic-secret-prop"
              value={secretProperty}
              onChange={(e) => setSecretProperty(e.target.value)}
              placeholder={DEFAULT_SECRET_PROPERTY}
            />
            <p className="text-xs text-muted-foreground">
              Connector property populated from the selected secret. Ignored
              if no secret is selected above.
            </p>
          </div>
        </>
      )}

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {!readOnlyWarehouse && (
        <div className="flex justify-end">
          <Button type="submit" disabled={pending || !isValid()}>
            {pending && <Loader2 className="h-4 w-4 animate-spin" />}
            {mode === "add" ? "Add catalog" : "Save changes"}
          </Button>
        </div>
      )}
    </form>
  );
}
