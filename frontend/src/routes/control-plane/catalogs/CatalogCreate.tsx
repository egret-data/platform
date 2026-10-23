import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import { useControlPlaneCatalogRegistryCreateRegistryCatalog } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import {
  type CreateWarehouseInput,
  type Warehouse,
  createWarehouse,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { ConfigSection } from "@/components/form/ConfigSection";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { FieldLabel } from "../config/FieldLabel";
import { warehousesQueryKey } from "../queryKeys";
import { catalogsQueryKey } from "./Catalogs";
import {
  AdvancedEditor,
  type AdvancedIssue,
  type AdvancedValue,
  parseAdvancedIssue,
} from "./kinds/AdvancedEditor";
import {
  EgretWarehouseForm,
  GENERIC_REQUIRED_KEY,
  KindForm,
  cleanAdvanced,
  cleanConfig,
  emptyConfigFor,
  submitProblem,
} from "./kinds/forms";
import { EGRET_WAREHOUSE_KIND, KIND_OPTIONS } from "./kinds/pickers";
import { PageContainer } from "@/components/layout/PageContainer";
import { DetailHeader } from "@/components/layout/DetailHeader";

/** `CreateRegistryCatalogBody.name`, and `is_valid_catalog_name` behind it. */
const NAME_RE = /^[a-z][a-z0-9_]{0,62}$/;
const NAME_RULE =
  "A catalog name must match ^[a-z][a-z0-9_]{0,62}$ — it is the SQL name on every engine the catalog is ever attached to, which is why it cannot be edited later.";

/**
 * Create a catalog: choose a kind, fill that kind's typed form, optionally add
 * per-engine advanced properties.
 *
 * One page, three states (no kind / a registry kind / egret's own warehouse),
 * not a wizard — every field a kind needs fits on one screen, and a wizard
 * would hide the name rule behind a step.
 */
export default function CatalogCreate() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const [kind, setKind] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [config, setConfig] = useState<Record<string, unknown>>({});
  const [advanced, setAdvanced] = useState<AdvancedValue>({});
  // The egret-warehouse branch's own two fields; they are not `config`.
  const [workspaceId, setWorkspaceId] = useState("");
  const [storageId, setStorageId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [issues, setIssues] = useState<AdvancedIssue[]>([]);
  // Owned here because the page, not the section, knows when it must be open:
  // on the `generic` kind (its required key lives in there) and on a refusal
  // that names one of its keys.
  const [advancedOpen, setAdvancedOpen] = useState(false);

  const isWarehouse = kind === EGRET_WAREHOUSE_KIND;
  const workspaces = useMyWorkspaces(isWarehouse);

  // `<ApiError>`: `apiFetch` THROWS on non-2xx, so the value react-query hands
  // an error handler is an `ApiError` carrying `.message` — the generated
  // `TError = ErrorBody` would make `.error` type-check and be `undefined`
  // at runtime, i.e. a create failure with nothing on screen.
  const createMut = useControlPlaneCatalogRegistryCreateRegistryCatalog<ApiError>();
  const warehouseMut = useMutation<Warehouse, ApiError, CreateWarehouseInput>({
    mutationFn: createWarehouse,
  });
  const pending = createMut.isPending || warehouseMut.isPending;

  function chooseKind(next: string) {
    setKind(next);
    setError(null);
    setIssues([]);
    setAdvanced({});
    setAdvancedOpen(next === "generic");
    // Replaced, never merged: the previous kind's keys are unknown fields to
    // the next kind's parser, and `deny_unknown_fields` refuses the body.
    setConfig(next === "" || next === EGRET_WAREHOUSE_KIND ? {} : emptyConfigFor(next));
  }

  const trimmed = name.trim();
  const nameProblem =
    kind === ""
      ? null
      : trimmed === ""
        ? "A name is required."
        : // The warehouse branch's name is a WAREHOUSE name, which egret
          // normalises into a catalog name (`normalize_catalog_name`,
          // `store.rs:553-555`) — so the SQL rule below does not apply to it.
          !isWarehouse && !NAME_RE.test(trimmed)
          ? NAME_RULE
          : null;
  /**
   * The configuration as it will be POSTed — and, because it is the same
   * value, as it is validated.
   *
   * These were two different values for one round: `submitProblem` read the
   * raw form state while `cleanConfig` posted the trimmed, blank-stripped one.
   * That disagrees in both directions — a URI with a leading space was refused
   * for a character the page was about to remove, and a `user` of `"  "`
   * satisfied the "user and password go together" check and then had the user
   * dropped, so the backend refused the body instead. Validating the value
   * that is sent is the only shape in which neither can recur (SP3c review,
   * fix round 2, N1).
   */
  const posted =
    kind === "" || isWarehouse ? {} : cleanConfig(kind, config);
  const configProblem = isWarehouse
    ? workspaceId === ""
      ? "Select a workspace."
      : storageId === ""
        ? "Select a storage connection."
        : null
    : kind === ""
      ? "Choose a kind."
      : submitProblem(kind, posted, asMaps(advanced));
  const canCreate = kind !== "" && !nameProblem && !configProblem && !pending;

  async function handleCreate() {
    setError(null);
    setIssues([]);
    try {
      if (isWarehouse) {
        // The registry REFUSES this kind: `create_registry_catalog` answers 400
        // "an egret warehouse catalog is created with its warehouse"
        // (`catalogs.rs:318-322`). That is not a gap to work around — creating
        // the warehouse is what creates the registry catalog, because SP3a's
        // warehouse-create path writes the `registry.catalogs` row with
        // `warehouse_id` set. SP3c D5 keeps this endpoint for exactly this.
        //
        // `CreateWarehouseInput` is `{ name, workspaceId, storageConnectionId }`;
        // the `Warehouse` that comes back spells the same field `storageConnId`.
        // Both are `string`, so TypeScript would not catch the swap.
        const wh = await warehouseMut.mutateAsync({
          name: trimmed,
          workspaceId,
          storageConnectionId: storageId,
        });
        await queryClient.invalidateQueries({ queryKey: warehousesQueryKey });
        await queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
        // The warehouse endpoint returns the WAREHOUSE, not the catalog it
        // created, and its id is not the catalog's — so there is no detail
        // page to go to. The list is where the new catalog appears.
        toast.success(`Warehouse ${wh.name} created`);
        navigate("/control-plane/catalogs");
        return;
      }
      const created = await createMut.mutateAsync({
        data: {
          name: trimmed,
          kind,
          // `posted`, not a second `cleanConfig(...)` call: the value the
          // button's own enabled state was computed from is the value sent.
          config: posted,
          ...withAdvanced(kind, posted, advanced),
          ...(description.trim() ? { description: description.trim() } : {}),
        },
      });
      await queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
      // Always true at runtime (`apiFetch` throws on every other status); this
      // is what narrows `data` off the generated response union.
      if (created.status === 201) navigate(`/control-plane/catalogs/${created.data.id}`);
    } catch (e) {
      const message = (e as ApiError).message ?? "Failed to create the catalog.";
      setError(message);
      const issue = parseAdvancedIssue(message);
      setIssues(issue ? [issue] : []);
      if (issue) setAdvancedOpen(true);
    }
  }

  const option = KIND_OPTIONS.find((o) => o.kind === kind);

  return (
    <PageContainer width="narrow" className="space-y-4">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/catalogs", label: "Catalogs" }}
        title="New catalog"
        description={
          <>
            A catalog is a set of tables an engine can query. Choose what it reads, then attach it
            to an engine and assign it to a workspace from its page.
          </>
        }
      />

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <ConfigSection
        level={2}
        title="Kind"
        description="What this catalog reads. It cannot be changed afterwards — the engines read a different set of properties for each."
      >
        <div className="space-y-1.5 sm:max-w-md">
          <FieldLabel
            htmlFor="cc-kind"
            label="Kind"
            help="Each kind has its own typed form below. egret writes the properties every attached engine needs from what you fill in."
          />
          <Select value={kind} onValueChange={chooseKind}>
            <SelectTrigger id="cc-kind" className="w-full">
              <SelectValue placeholder="Select a kind" />
            </SelectTrigger>
            <SelectContent>
              {KIND_OPTIONS.map((o) => (
                <SelectItem key={o.kind} value={o.kind}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {option && <p className="text-xs text-muted-foreground">{option.blurb}</p>}
        </div>
      </ConfigSection>

      {kind !== "" && (
        <>
          <ConfigSection
            level={2}
            title="Identity"
            description="The name is how the catalog is addressed in SQL on every engine."
          >
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div className="space-y-1.5">
                <FieldLabel
                  htmlFor="cc-name"
                  label="Name"
                  help={
                    isWarehouse
                      ? "The warehouse name. egret derives the catalog's SQL name from it, lowercasing and replacing anything that is not a letter, digit or underscore."
                      : NAME_RULE
                  }
                />
                <Input
                  id="cc-name"
                  value={name}
                  placeholder="e.g. analytics"
                  onChange={(e) => setName(e.target.value)}
                />
                {nameProblem && name !== "" && (
                  <p className="text-xs text-destructive">{nameProblem}</p>
                )}
              </div>
              {!isWarehouse && (
                <div className="space-y-1.5">
                  <FieldLabel
                    htmlFor="cc-description"
                    label="Description"
                    help="Optional. Shown wherever the catalog is listed."
                  />
                  <Textarea
                    id="cc-description"
                    value={description}
                    rows={2}
                    onChange={(e) => setDescription(e.target.value)}
                  />
                </div>
              )}
            </div>
          </ConfigSection>

          {isWarehouse ? (
            <EgretWarehouseForm
              workspaces={workspaces.data ?? []}
              workspaceId={workspaceId}
              onWorkspaceId={setWorkspaceId}
              storageId={storageId}
              onStorageId={setStorageId}
            />
          ) : (
            <>
              <KindForm kind={kind} config={config} onChange={setConfig} />
              {/* Not for the warehouse branch: `createWarehouse` takes no
                  advanced properties, and the catalog it creates is egret's
                  own to render. */}
              <AdvancedEditor
                value={advanced}
                onChange={setAdvanced}
                issues={issues}
                requiredKeys={kind === "generic" ? GENERIC_REQUIRED_KEY : undefined}
                open={advancedOpen}
                onOpenChange={setAdvancedOpen}
              />
            </>
          )}
        </>
      )}

      <div className="flex flex-wrap items-center justify-end gap-3 border-t pt-4">
        {(nameProblem || configProblem) && (
          <p role="status" className="mr-auto text-xs text-muted-foreground">
            {/* The rule itself is printed once, under the field it is about;
                repeating it here would put the same 30 words on screen twice. */}
            {nameProblem
              ? trimmed === ""
                ? nameProblem
                : "The name does not match the rule above."
              : configProblem}
          </p>
        )}
        <Button variant="outline" onClick={() => navigate("/control-plane/catalogs")}>
          Cancel
        </Button>
        <Button onClick={handleCreate} disabled={!canCreate}>
          {pending && <Loader2 className="h-4 w-4 animate-spin" />}
          Create catalog
        </Button>
      </div>
    </PageContainer>
  );
}

/** `AdvancedValue` as the plain per-engine maps the helpers take. */
function asMaps(a: AdvancedValue): Record<string, Record<string, string>> {
  return { ...(a.trino ? { trino: a.trino } : {}), ...(a.starrocks ? { starrocks: a.starrocks } : {}) };
}

/**
 * `advanced`, or nothing at all. The body's field defaults to `null` on the
 * server, which `CatalogDef::parse` reads as "no advanced properties" — so an
 * empty object is noise rather than information.
 */
function withAdvanced(
  kind: string,
  config: Record<string, unknown>,
  advanced: AdvancedValue,
): { advanced?: Record<string, Record<string, string>> } {
  const cleaned = cleanAdvanced(kind, config, asMaps(advanced));
  return Object.keys(cleaned).length > 0 ? { advanced: cleaned } : {};
}
