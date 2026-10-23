import { useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";

import {
  type CreateEngineInput,
  type Engine,
  type Secret,
  type Warehouse,
  createEngine,
  listSecrets,
  listWarehouses,
} from "@/api/controlPlane";
import type { CreateEngineResponse, RegistryCatalogDto } from "@/api/generated/model";
import {
  useControlPlaneCatalogRegistryListRegistryCatalogs,
  type controlPlaneCatalogRegistryListRegistryCatalogsResponse,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { ApiError } from "@/api/client";
import { pickableSecrets } from "@/lib/systemSecrets";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
  DEFAULT_TRINO_IMAGE_VERSION,
  TRINO_FIELD_DEFAULTS,
  enginesQueryKey,
} from "./ControlPlaneEngines";
import { TrinoConfigEditor } from "./EngineDetail";
import { warehousesQueryKey } from "./queryKeys";
import { useEngineConfigForm } from "./useEngineConfigForm";
import type { StarRocksDelegationOption } from "./useStarRocksConfigForm";
import { PageContainer } from "@/components/layout/PageContainer";
import { DetailHeader } from "@/components/layout/DetailHeader";

/**
 * A registry catalog can be ATTACHED here whatever its kind, but only an egret
 * warehouse catalog can LEAD. SP3c-2 dropped `engines.warehouse_id`, so the
 * reason is no longer "a NOT NULL column has to be filled" — it is that real
 * cluster state is derived from the primary attachment's warehouse:
 * `ensure_engine_role` seeds the engine's Ranger role from it, and a StarRocks
 * engine derives its builtin storage volume from it. A catalog with no
 * warehouse can answer neither, so the server refuses it as primary.
 *
 * So the row disables its radio and says why, in the server's own terms. The
 * alternative — offering a radio the server refuses — spends a round trip and a
 * 400 to say something the row already knows (design D2).
 */
const canBePrimary = (c: RegistryCatalogDto) => c.warehouseId != null;

/**
 * Full-page create-engine wizard (`/control-plane/engines/new`). Mirrors the
 * detail Settings layout: identity header + catalogs, then the SAME left-nav
 * config editor (Trino) so an engine is configured with the full Trino helm
 * values surface at creation time. StarRocks keeps the compact create fields.
 *
 * SP3c-1 replaced the single warehouse `<Select>` with an attach list: Task 2
 * widened `CreateEngineBody` so the create body carries the attachments, and
 * the server attaches them inside the create it already performs. ONE request
 * with ONE outcome — the shape this screen would otherwise have (create, then
 * an `attachCatalog` per extra catalog from the browser) leaves a half-attached
 * engine behind any tab closed mid-loop, and fires several config applies at a
 * cluster that is still provisioning.
 */
export default function EngineCreate() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [sp] = useSearchParams();
  const [engineType, setEngineType] = useState(
    sp.get("type") === "starrocks" ? "starrocks" : "trino",
  );
  const isStarRocks = engineType === "starrocks";

  // `warehousesQueryKey`, not a local `["cp","warehouses"]` literal: the value
  // this page used matched no other page's, so a warehouse deleted elsewhere
  // (today: Control Plane → Catalogs, on an egret-warehouse row) left this list
  // stale for `staleTime` with every test green. The rows are only read for the "provisioned but not registered"
  // note below — the attachable set comes from the registry.
  const warehouses = useQuery<Warehouse[], ApiError>({
    queryKey: warehousesQueryKey,
    queryFn: listWarehouses,
    retry: false,
  });
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: ["cp", "secrets"],
    queryFn: listSecrets,
    retry: false,
  });
  // `<…, ApiError>`: the thrown value is an `ApiError` with `.message` and
  // `.status`; the generated `TError = ErrorBody` would make `.error`
  // type-check and be `undefined` at runtime.
  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs<
    controlPlaneCatalogRegistryListRegistryCatalogsResponse,
    ApiError
  >(undefined, { query: { retry: false } });
  const allCatalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];

  const createMutation = useMutation<CreateEngineResponse, ApiError, CreateEngineInput>({
    mutationFn: createEngine,
    // Navigation is NOT here: an engine that came up without some of its
    // catalogs has something to say first, and `handleCreate` decides.
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: enginesQueryKey });
    },
  });

  const [name, setName] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [primaryCatalogId, setPrimaryCatalogId] = useState("");
  const [tlsCertSecretRef, setTlsCertSecretRef] = useState("");
  const [rawInvalid, setRawInvalid] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * Set only when the engine EXISTS and some of the body's `catalogs` are not
   * on it. Holding it here rather than navigating straight through is what
   * makes `catalogAttachErrors` reach a human: the field is the whole reason
   * `CreateEngineResponse` is its own type, and a screen that never reads it is
   * the failure that type was added to prevent.
   */
  const [created, setCreated] = useState<CreateEngineResponse | null>(null);

  // StarRocks-only compact fields.
  // Blank = omitted from the body = the server's pinned default (shown as the
  // placeholder). Only an image the operator types is sent.
  const [feImage, setFeImage] = useState("");
  const [cnImage, setCnImage] = useState("");
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
  const tlsSecrets = pickableSecrets(secrets.data ?? []).filter(
    (s) => s.type === "kubernetes.io/tls",
  );

  /**
   * Provisioned warehouses the registry has no catalog for. A warehouse gets
   * one when it is created, and engine create calls the same idempotent ensure
   * again — but that ensure is explicitly non-fatal (it logs and moves on), so
   * a warehouse CAN exist with no catalog, and it is then simply absent from
   * the list above with nothing said. (The boot backfill was a third chance at
   * this until SP3c-2 Task 1 deleted it.)
   *
   * Computed only when the registry list actually ANSWERED. A 403 or a pending
   * read leaves `allCatalogs` empty, and "every warehouse is unregistered" is
   * a claim about the registry that a failed read cannot support.
   */
  const unregistered =
    catalogsQ.data?.status === 200
      ? provisioned.filter((w) => !allCatalogs.some((c) => c.warehouseId === w.id))
      : [];

  const nameOf = (catalogId: string) =>
    allCatalogs.find((c) => c.id === catalogId)?.name ?? catalogId;

  function toggleAttach(id: string, on: boolean) {
    setSelected((s) => (on ? (s.includes(id) ? s : [...s, id]) : s.filter((x) => x !== id)));
    // A primary that is not attached is not a state the body can express, so
    // un-attaching the primary clears it rather than leaving a submit that the
    // server would have to refuse.
    if (!on) setPrimaryCatalogId((p) => (p === id ? "" : p));
  }

  function markPrimary(id: string) {
    setPrimaryCatalogId(id);
    // …and the mirror: marking a row primary attaches it.
    setSelected((s) => (s.includes(id) ? s : [...s, id]));
  }

  const canCreate =
    name.trim().length > 0 &&
    primaryCatalogId.length > 0 &&
    !rawInvalid &&
    // An ingress without a host was dropped silently (KI-200); the host field
    // says why Create is blocked.
    (isStarRocks || !cfg.ingressHostMissing) &&
    !createMutation.isPending &&
    // The engine already exists; a second click would be a 409 at best.
    created === null;

  /**
   * Turn the server's id-shaped refusals into something an operator can act on.
   *
   * `resolve_primary_catalog` resolves EVERY id in the body before anything is
   * provisioned or minted, so an id this list still shows but the server cannot
   * find means the row went away between the read and the submit — and the
   * important half of that sentence is that nothing was created. Left as the
   * raw 400 it is a bare uuid and no indication of whether a cluster is now
   * running.
   *
   * THE MATCHED STRING IS THE BACKEND'S, AND NOTHING LINKS THE TWO (review
   * F-4). It is built at `crates/egret-engine-mgmt/src/engines.rs:2822` and
   * pinned there by `crates/egret-engine-mgmt/tests/routes.rs:9249`, which
   * asserts `"bad request: no such catalog {id}"` verbatim. So a reword goes
   * red on the Rust side, gets updated there, and this match silently stops
   * firing — the fallback is safe (the server's own message is passed through
   * untouched) but the "nothing was provisioned" reassurance is lost, and no
   * test on either side would notice. If you are reading this because you
   * changed that message: change this regex too. The durable fix is the shared
   * fixture Task 5 already carries.
   */
  function explain(message: string | undefined): string {
    if (!message) return "Failed to provision engine.";
    const m =
      /no such catalog ([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i.exec(
        message,
      );
    if (!m) return message;
    return (
      `${message} — that is «${nameOf(m[1])}», which this list still shows. ` +
      `Nothing was provisioned: egret resolves every catalog in the request before it ` +
      `creates anything. Reload the catalog list and try again.`
    );
  }

  async function handleCreate() {
    setError(null);
    // Belt and braces with `canCreate`: the body cannot express a primary that
    // is not attached, so it is built from the primary outwards.
    if (!primaryCatalogId) return;
    const catalogs = [
      { catalogId: primaryCatalogId, primary: true },
      ...selected
        .filter((id) => id !== primaryCatalogId)
        .map((id) => ({ catalogId: id, primary: false })),
    ];
    // No `warehouseId`: the server derives it from whichever catalog is
    // primary, and a body carrying BOTH is refused rather than having one of
    // them silently ignored.
    const input: CreateEngineInput = isStarRocks
      ? {
          name: name.trim(),
          catalogs,
          kind: "starrocks",
          starrocks: {
            ...(feImage.trim() ? { feImage: feImage.trim() } : {}),
            ...(cnImage.trim() ? { cnImage: cnImage.trim() } : {}),
            feReplicas,
            cnReplicas,
            ...(delegationMode !== "" ? { delegationMode, tlsEnabled } : {}),
          },
          ...TRINO_FIELD_DEFAULTS,
        }
      : {
          name: name.trim(),
          catalogs,
          ...TRINO_FIELD_DEFAULTS,
          trinoConfig: cfg.builtTrinoConfig,
          ...(tlsCertSecretRef ? { tlsCertSecretRef } : {}),
        };
    try {
      const engine = await createMutation.mutateAsync(input);
      // `?? []` covers a server that predates the field; the point of the
      // non-cast return type is that when the field IS sent it arrives.
      if ((engine.catalogAttachErrors ?? []).length === 0) {
        navigate(`/control-plane/engines/${engine.id}`);
        return;
      }
      setCreated(engine);
    } catch (err) {
      setError(explain((err as ApiError).message));
    }
  }

  return (
    <PageContainer width="narrow" className="space-y-4">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/engines", label: "Engines" }}
        title="New engine"
        description={
          <>
            {isStarRocks
              ? "Provision a StarRocks cluster against one or more registry catalogs and set its FE/CN sizing and delegation up front."
              : "Provision a query engine against one or more registry catalogs and configure it with the full Trino config surface up front."}
          </>
        }
      />

      {error && (
        <Alert variant="destructive" data-testid="ec-error">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {/*
        WHAT IS IN THIS PANEL HAS NO ROW ON THE ENGINE, WHICH IS WHY IT SAYS
        "attach again" AND NOT "Retry" (review F-1).

        `attach_to_engine` splits by engine kind, and the split decides where a
        failure becomes visible (`catalog_apply.rs:1053-1160`):

          - **Trino applies first and writes after.** A failed apply
            `rollback_minted`s and returns `Err` BEFORE `insert_attachment`, so
            the catalog is reported here and the Catalogs tab has nothing to
            list for it — Task 9's `failed` badge and its Retry never appear.
            The pre-apply refusals (mint, warehouse, engine support) return
            earlier still, likewise with no row.
          - **StarRocks records first and converges after**, because its FE is
            unreachable for minutes after `create_engine` — exactly when this
            wizard attaches. A failed apply calls `set_apply_failed` and returns
            **`Ok(())`**, so it is NOT in `catalogAttachErrors` at all: the
            wizard navigates normally and the tab reports it with Retry.

        The two paths are complementary and the panel names its own half. The
        one exception is `ApplyError::Unrecorded` whose compensating
        `delete_attachment` itself fails (`:1118-1127`) — a row survives and is
        reported here too; "normally" below is doing that work.

        F-2: NOTHING PERSISTS THIS LIST. The server only `tracing::warn!`s
        (`engines.rs:2858-2866`), so this panel is the sole record and the
        "← Engines" link directly above it destroys it. The copy says so
        instead of letting the operator find out.

        THE BACKEND PIN, AND WHAT IT DOES NOT COVER (Task 10 review, carried
        to Task 11). The only test that ever enters this state is
        `crates/egret-engine-mgmt/tests/routes.rs:9323`
        (`a_catalog_that_cannot_attach_is_reported_on_the_created_engine`), and
        its vehicle is `check_engine_supports` — a PRE-APPLY refusal, the one
        arm where no row can exist because nothing had been recorded yet. The
        sentence above it, "an attach that fails this way normally leaves no row
        behind", is therefore asserted for exactly one of the two Trino arms:
        the other, a failed APPLY, is true today only because
        `catalog_apply.rs` writes after it applies. If SP3c-2 converges Trino
        onto the StarRocks record-first order, 9323 still passes — it never
        reaches an apply — while this paragraph becomes false and the operator
        is told to re-attach a catalog whose row is sitting on the Catalogs tab
        with a Retry. That PR owns the test that would notice; there is none
        here to change, which is why this note exists instead.
      */}
      {created && (
        <Alert data-testid="ec-attach-errors">
          <AlertDescription className="space-y-2">
            <p>
              «{created.name}» was created and is provisioning, but{" "}
              {created.catalogAttachErrors.length === 1
                ? "one catalog you attached is not on it"
                : `${created.catalogAttachErrors.length} of the catalogs you attached are not on it`}
              . The engine is not rolled back for them, and its primary catalog IS attached.
            </p>
            <ul className="list-disc space-y-1 pl-5">
              {created.catalogAttachErrors.map((e) => (
                <li key={e.catalogId}>
                  «{nameOf(e.catalogId)}» — {e.message}
                </li>
              ))}
            </ul>
            <p>
              {created.catalogAttachErrors.length === 1 ? "It is" : "They are"} not attached
              at all, and an attach that fails this way normally leaves no row behind — so
              the engine&apos;s Catalogs tab has nothing to Retry.{" "}
              {created.catalogAttachErrors.length === 1 ? "Attach it" : "Attach them"} again
              from the picker there.{" "}
              <strong>
                This list is not stored anywhere — leaving this page loses{" "}
                {created.catalogAttachErrors.length === 1 ? "the name" : "the names"}.
              </strong>
            </p>
            <p className="text-xs text-muted-foreground">
              A catalog whose attach failed after the engine had already recorded it — the
              StarRocks path — is not listed here: it shows as failed on that tab, with
              Retry.
            </p>
            <Button
              size="sm"
              onClick={() =>
                navigate(`/control-plane/engines/${created.id}?tab=catalogs`)
              }
            >
              Open «{created.name}» → Catalogs to attach{" "}
              {created.catalogAttachErrors.length === 1 ? "it" : "them"} again
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {/* Identity: type and name. */}
      <section className="space-y-4 rounded-lg border p-4">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
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
        </div>
      </section>

      {/*
        Catalogs. `id="ec-warehouse"` is DELIBERATELY the old warehouse
        `<Select>`'s id, moved onto the control that replaced it: Task 11
        rewrites the e2e spec that targets it, and changing the id and the spec
        in one pass would lose the ability to tell a selector break from a
        behaviour break.

        FOR TASK 11 (review F-3): the id no longer names a `<Select>` trigger,
        so `click #ec-warehouse` then pick an option does NOT work any more. It
        is the `RadioGroup` root `<div>`. Drive it with
        `#ec-warehouse [role="radio"]`, or by the per-row
        `#ec-primary-<catalogId>`; the attach checkboxes are
        `#ec-attach-<catalogId>`. `EngineCreate.test.tsx` asserts that the
        anchor exists AND that it contains one driveable radio per row, so a
        refactor that drops it goes red here rather than in Task 11's spec
        months later, where it would read like a behaviour change.
      */}
      <section className="space-y-3 rounded-lg border p-4">
        <div className="space-y-1">
          <h2 className="text-base font-semibold">Catalogs</h2>
          <p className="text-sm text-muted-foreground">
            Attach any number of registry catalogs, and mark one primary. The primary
            is the warehouse the engine is created against; the rest are attached in the
            same request, so the engine never comes up half-configured.
          </p>
        </div>

        {catalogsQ.isLoading && <Skeleton className="h-24 w-full" />}

        {catalogsQ.isError && (
          <Alert variant="destructive" data-testid="ec-catalogs-blocked">
            <AlertDescription>
              {catalogsQ.error?.status === 403
                ? "Choosing what an engine reads goes through the catalog registry, which requires the catalog_registry:read permission. Your account does not have it, so egret cannot offer you a catalog here and an engine cannot be created without one. A platform administrator can grant catalog_registry:read."
                : `Couldn't read the catalog registry: ${
                    catalogsQ.error?.message ?? "the request failed"
                  }. An engine needs a primary catalog, so creating one is blocked until it answers.`}
            </AlertDescription>
          </Alert>
        )}

        {!catalogsQ.isLoading && !catalogsQ.isError && allCatalogs.length === 0 && (
          <p className="text-sm text-muted-foreground">
            There are no catalogs to attach. Create one in Control Plane → Catalogs.
          </p>
        )}

        {allCatalogs.length > 0 && (
          <RadioGroup
            id="ec-warehouse"
            value={primaryCatalogId}
            onValueChange={markPrimary}
            className="gap-0 divide-y rounded-md border"
          >
            {allCatalogs.map((c) => (
              <div key={c.id} className="flex items-start gap-3 p-3">
                <Checkbox
                  className="mt-0.5"
                  id={`ec-attach-${c.id}`}
                  aria-label={`Attach ${c.name}`}
                  checked={selected.includes(c.id)}
                  onCheckedChange={(v) => toggleAttach(c.id, v === true)}
                />
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex flex-wrap items-center gap-2 text-sm">
                    <span className="font-medium">{c.name}</span>
                    <span className="text-xs text-muted-foreground">{c.kind}</span>
                  </div>
                  {!canBePrimary(c) && (
                    <p className="text-xs text-muted-foreground">
                      Can be attached, but not primary: the engine&apos;s Ranger role and
                      (for StarRocks) its storage are still derived from the primary
                      catalog&apos;s egret warehouse, and this catalog has none.
                    </p>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <RadioGroupItem
                    id={`ec-primary-${c.id}`}
                    value={c.id}
                    disabled={!canBePrimary(c)}
                    aria-label={`Make ${c.name} primary`}
                  />
                  <Label
                    htmlFor={`ec-primary-${c.id}`}
                    className="text-xs font-normal text-muted-foreground"
                  >
                    Primary
                  </Label>
                </div>
              </div>
            ))}
          </RadioGroup>
        )}

        {allCatalogs.length > 0 && primaryCatalogId === "" && (
          <p className="text-xs text-muted-foreground" data-testid="ec-needs-primary">
            Mark one catalog primary to create the engine.
          </p>
        )}

        {unregistered.length > 0 && (
          <p className="text-xs text-muted-foreground" data-testid="ec-unregistered-warehouses">
            {unregistered.map((w) => w.name).join(", ")}{" "}
            {unregistered.length === 1
              ? "is a provisioned warehouse with no registry catalog, so it is not listed above"
              : "are provisioned warehouses with no registry catalog, so they are not listed above"}
            . egret registers one when a warehouse is created and again at startup; until
            then it cannot be attached here.
          </p>
        )}
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
    </PageContainer>
  );
}
