import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";

import {
  type AddCatalogInput,
  type CatalogClient,
  type EngineCatalog,
  type EngineCatalogDetail,
  type Secret,
  type Warehouse,
  addEngineCatalog,
  listClients,
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
import { connectorByName, connectors } from "./config/connectorCatalog";
import { SecretOrValueList } from "./config/SecretOrValueList";

const OTHER_SENTINEL = "__other__";
// shadcn Select can't use "" as a SelectItem value, but "" (no clientId — mint a
// dedicated client under the default engine role) is the valid default state.
// Map that state to this sentinel in the Select and back to "" in onValueChange.
const DEFAULT_CLIENT_SENTINEL = "__default__";

export interface CatalogFormProps {
  mode: "add" | "edit";
  engineId: string;
  /** Edit only: the fetched detail to prefill from (kind + redacted config + secretRef). */
  initial?: EngineCatalogDetail;
  /** Called after a successful add/update — the parent should invalidate its catalog queries. */
  onSubmitted: () => void;
}

/**
 * The catalog add/edit form: connector-first — a single Connector picker
 * (egret warehouse / curated connector / custom name) selects `kind`, and
 * every non-warehouse kind gets the same uniform property editor
 * (`SecretOrValueList`), where each property is either a typed value or a
 * reference to an egret secret. Config is stored as
 * `{ properties, secretProperties }` (v2) rather than v1's per-kind shape.
 *
 * In `edit` mode `name`/kind are immutable (rendered as static text) and an
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
  const secretsQuery = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });
  const secretOptions = (secretsQuery.data ?? []).map((s) => ({
    id: s.id,
    name: s.name,
  }));

  const [name, setName] = useState(initial?.name ?? "");
  const [kind, setKind] = useState(initial?.kind ?? "egret-warehouse");
  const [error, setError] = useState<string | null>(null);

  // Add mode only: has the user picked "Other (custom)…"? Drives whether the
  // free-text connector-name input is shown instead of the Select.
  const [customConnector, setCustomConnector] = useState(false);

  // egret-warehouse (add only — edit is read-only, config is system-managed)
  const [warehouseId, setWarehouseId] = useState("");
  // "" = default (auto-create a dedicated client under the engine's default
  // role); otherwise the id of an existing client of `warehouseId` whose role
  // the newly-minted client should inherit (see handleSubmit).
  const [clientId, setClientId] = useState("");
  const clientsQuery = useQuery<CatalogClient[], ApiError>({
    queryKey: ["cp", "warehouse-clients", warehouseId],
    queryFn: () => listClients(warehouseId),
    enabled: warehouseId !== "",
    retry: false,
  });

  // All other connectors: uniform property/secret maps, keyed by property
  // name. Kept disjoint — a key lives in exactly one of the two maps.
  const [values, setValues] = useState<Record<string, string>>(() =>
    mode === "edit"
      ? ((initial?.config?.properties as Record<string, string> | undefined) ??
        {})
      : {},
  );
  const [secrets, setSecrets] = useState<Record<string, string>>(() =>
    mode === "edit"
      ? ((initial?.config?.secretProperties as
          | Record<string, string>
          | undefined) ?? {})
      : {},
  );

  const addMutation = useMutation<EngineCatalog, ApiError, AddCatalogInput>({
    mutationFn: (input) => addEngineCatalog(engineId, input),
  });
  const updateMutation = useMutation<
    EngineCatalog,
    ApiError,
    { config: Record<string, unknown> }
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
  // tpch / tpcds carry no configurable properties, so editing one is a no-op.
  const noEditableConfig =
    mode === "edit" && (kind === "tpch" || kind === "tpcds");
  // A catalog stored in the v1 config shape ({host,port,…} old postgres, or
  // {connectorName,properties,secretProperty} old generic). The v2 editor only
  // understands {properties, secretProperties}, so editing a legacy row would
  // silently drop its config on save — make it read-only instead. tpch/tpcds
  // have empty config with no `properties` key too, but are handled above; the
  // `!noEditableConfig` guard keeps them out of this flag.
  const legacyFormat =
    mode === "edit" &&
    kind !== "egret-warehouse" &&
    !noEditableConfig &&
    (kind === "generic" ||
      (initial?.config != null &&
        typeof initial.config === "object" &&
        !("properties" in (initial.config as object))));

  // Set a plain-value property — clears the same key from `secrets` so the
  // two maps stay disjoint (SecretOrValueList relies on this invariant).
  function setVal(key: string, value: string) {
    setValues((prev) => ({ ...prev, [key]: value }));
    setSecrets((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }

  // Set a secret-backed property — clears the same key from `values`.
  function setSec(key: string, secretId: string) {
    setSecrets((prev) => ({ ...prev, [key]: secretId }));
    setValues((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }

  function removeKey(key: string) {
    setValues((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
    setSecrets((prev) => {
      if (!(key in prev)) return prev;
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }

  function handleWarehouseChange(v: string) {
    // The client list is scoped to the warehouse, so a client picked for the
    // previous warehouse is meaningless here — reset to the default.
    setWarehouseId(v);
    setClientId("");
  }

  function handleConnectorChange(v: string) {
    // Switching connector changes the applicable property set, so clear any
    // properties/secrets entered for the previous one.
    setValues({});
    setSecrets({});
    setWarehouseId("");
    setClientId("");
    if (v === OTHER_SENTINEL) {
      setCustomConnector(true);
      setKind("");
    } else {
      setCustomConnector(false);
      setKind(v);
    }
  }

  function isValid(): boolean {
    if (mode === "add") {
      if (!name.trim()) return false;
      if (kind === "egret-warehouse") return warehouseId !== "";
      return kind.trim() !== "";
    }
    return true;
  }

  function resetAfterAdd() {
    setName("");
    setKind("egret-warehouse");
    setCustomConnector(false);
    setError(null);
    setWarehouseId("");
    setClientId("");
    setValues({});
    setSecrets({});
    addMutation.reset();
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const config: Record<string, unknown> =
      kind === "egret-warehouse"
        ? clientId
          ? { warehouseId, clientId }
          : { warehouseId }
        : {
            properties: values,
            // Drop rows toggled to "secret" but left unpicked (empty id).
            secretProperties: Object.fromEntries(
              Object.entries(secrets).filter(([, id]) => id !== ""),
            ),
          };

    try {
      if (mode === "add") {
        const input: AddCatalogInput = {
          name: name.trim(),
          kind: kind.trim(),
          config,
        };
        await addMutation.mutateAsync(input);
        resetAfterAdd();
      } else {
        await updateMutation.mutateAsync({ config });
      }
      onSubmitted();
    } catch (err) {
      setError(
        (err as ApiError).message ??
          `Failed to ${mode === "add" ? "add" : "update"} catalog.`,
      );
    }
  }

  const selectedConnector = connectorByName[kind];

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
        <Label htmlFor="cat-connector">Connector</Label>
        {mode === "add" ? (
          <>
            <Select
              value={
                customConnector
                  ? OTHER_SENTINEL
                  : kind === "egret-warehouse" || connectorByName[kind]
                    ? kind
                    : ""
              }
              onValueChange={handleConnectorChange}
            >
              <SelectTrigger id="cat-connector" className="w-full">
                <SelectValue placeholder="Select a connector" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="egret-warehouse">
                  Egret warehouse
                </SelectItem>
                {connectors.map((c) => (
                  <SelectItem key={c.name} value={c.name}>
                    {c.label}
                  </SelectItem>
                ))}
                <SelectItem value={OTHER_SENTINEL}>
                  Other (custom)…
                </SelectItem>
              </SelectContent>
            </Select>
            {customConnector && (
              <Input
                value={kind}
                onChange={(e) => setKind(e.target.value)}
                placeholder="e.g. redshift"
              />
            )}
          </>
        ) : (
          <p id="cat-connector" className="text-sm text-muted-foreground">
            {kind === "egret-warehouse"
              ? "Egret warehouse"
              : (selectedConnector?.label ?? kind)}
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
          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cat-warehouse">Warehouse</Label>
              <Select value={warehouseId} onValueChange={handleWarehouseChange}>
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

            <div className="space-y-1.5">
              <Label htmlFor="cat-client">Client</Label>
              <Select
                value={clientId === "" ? DEFAULT_CLIENT_SENTINEL : clientId}
                onValueChange={(v) =>
                  setClientId(v === DEFAULT_CLIENT_SENTINEL ? "" : v)
                }
                disabled={warehouseId === ""}
              >
                <SelectTrigger id="cat-client" className="w-full">
                  <SelectValue placeholder="Select a warehouse first" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={DEFAULT_CLIENT_SENTINEL}>
                    Create a dedicated client (default)
                  </SelectItem>
                  {(clientsQuery.data ?? []).map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {c.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                Selecting a client runs this catalog under that client&rsquo;s
                role — a new dedicated client is minted under the same role.
                The default mints a dedicated client under the engine&rsquo;s
                default role instead.
              </p>
            </div>
          </div>
        ))}

      {legacyFormat && (
        <div className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
          This catalog uses an older configuration format. Remove and re-add it
          to edit its properties.
        </div>
      )}

      {kind !== "egret-warehouse" && !noEditableConfig && !legacyFormat && (
        <div className="space-y-1.5">
          <Label>Properties</Label>
          <SecretOrValueList
            catalog={selectedConnector?.props ?? []}
            catalogByKey={Object.fromEntries(
              (selectedConnector?.props ?? []).map((p) => [p.key, p]),
            )}
            values={values}
            secrets={secrets}
            secretOptions={secretOptions}
            setValue={setVal}
            setSecret={setSec}
            remove={removeKey}
          />
        </div>
      )}

      {noEditableConfig && (
        <div className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
          This catalog has no configurable properties.
        </div>
      )}

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {!readOnlyWarehouse && !noEditableConfig && !legacyFormat && (
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
