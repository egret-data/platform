import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, MoreHorizontal, Server, Trash2, Unlink } from "lucide-react";
import { toast } from "sonner";

import { ApiError } from "@/api/client";
import { type Engine, deleteEngine } from "@/api/controlPlane";
import { type Profile, listProfiles } from "@/api/policies";
import {
  type controlPlaneCatalogRegistryListRegistryCatalogsResponse,
  useControlPlaneCatalogRegistryDetachCatalog,
  useControlPlaneCatalogRegistryListRegistryCatalogs,
  useControlPlaneCatalogRegistryRegistryAttachCatalog,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import {
  getControlPlaneEnginesRangerPluginConfigQueryKey,
  useControlPlaneEnginesUpdateExternalEngine,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import type { UpdateExternalEngine } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { enginesQueryKey } from "./ControlPlaneEngines";
import { EngineDependentsTable, dependentsFromError } from "./EngineDependentsTable";
import { EnginePluginConfig } from "./EnginePluginConfig";
import {
  ENGINE_TYPE_LABEL,
  ExternalChip,
  PLUGIN_VERSION_RE,
  OpenInEnginePolicyLink,
  PolicyProfileLink,
  ServerRefusal,
  UnmodelledVersionNote,
  profilesQueryKey,
  useCan,
  ModelledPluginsUnavailable,
  useModelledPlugins,
} from "./externalEngine";
import { catalogKey, catalogsQueryKey, engineDetailKey } from "./queryKeys";
import { RangerCredentialSection, useRotationToken } from "./RangerCredential";

const OTHER = "__other__";
const NO_PROFILE = "__none__";
const TABS = new Set(["overview", "catalogs", "plugin-config", "settings"]);

/** The external engine row; `status` is the placeholder egret does not read, so it is left out. */
type ExternalEngine = Omit<Engine, "status">;

/**
 * The engine detail page for an `origin: external` engine.
 *
 * egret holds a row for a cluster it never provisions, so nothing that touches a
 * cluster is offered here: no pods, logs, terminal, monitor, autoscaling, config
 * editors, restart or secret rotation (the server refuses all of them with 409).
 * What is left is the record (address, plugin version), the profile bound to it,
 * the catalogs declared attached, the Ranger plugin configuration to install, and
 * the Ranger credential the plugin presents (rotated and retired on Overview).
 */
export default function ExternalEngineDetail({ engine }: { engine: ExternalEngine }) {
  const [sp, setSp] = useSearchParams();
  const raw = sp.get("tab");
  const tab = raw && TABS.has(raw) ? raw : "overview";
  const setTab = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", v);
        return n;
      },
      { replace: true },
    );

  const profilesQ = useQuery<Profile[], ApiError>({
    queryKey: profilesQueryKey,
    queryFn: listProfiles,
    retry: false,
  });
  const bound = profilesQ.data?.find((p) => p.engines.some((e) => e.id === engine.id));
  // Held here, above the tabs: the Overview panel unmounts while the operator
  // installs the new credential from the Plugin config tab.
  const rotation = useRotationToken(engine.id);

  return (
    <div className="space-y-5">
      <Link
        to="/control-plane/engines"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Engines
      </Link>

      <div className="flex flex-wrap items-start gap-3">
        <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <Server className="h-6 w-6" />
        </div>
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-2xl font-semibold leading-tight">{engine.name}</h1>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Badge variant="secondary">{engine.type}</Badge>
          <ExternalChip />
          <OpenInEnginePolicyLink engineId={engine.id} />
          <ExternalEngineActions engine={engine} />
        </div>
      </div>

      <Tabs value={tab} onValueChange={setTab} className="space-y-5">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="catalogs">Catalogs</TabsTrigger>
          <TabsTrigger value="plugin-config">Plugin config</TabsTrigger>
          <TabsTrigger value="settings">Settings</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="space-y-4">
          <Overview engine={engine} bound={bound} profilesQ={profilesQ} />
          <RangerCredentialSection engine={engine} rotation={rotation} />
        </TabsContent>
        <TabsContent value="catalogs">
          <DeclaredCatalogs engine={engine} />
        </TabsContent>
        <TabsContent value="plugin-config">
          <EnginePluginConfig engineId={engine.id} />
        </TabsContent>
        <TabsContent value="settings">
          <ExternalSettings engine={engine} profilesQ={profilesQ} bound={bound} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

/** Delete is the one lifecycle action an external engine keeps: egret removes its record only. */
function ExternalEngineActions({ engine }: { engine: ExternalEngine }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const canDelete = useCan("engine:delete");
  const [open, setOpen] = useState(false);
  const [blockedBy, setBlockedBy] = useState<EngineDependentDto[] | null>(null);

  const del = useMutation<void, ApiError, string>({
    mutationFn: deleteEngine,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: enginesQueryKey });
      void queryClient.invalidateQueries({ queryKey: profilesQueryKey });
      navigate("/control-plane/engines");
    },
    onError: (e) => {
      const dependents = dependentsFromError(e);
      if (dependents) {
        setBlockedBy(dependents);
        return;
      }
      toast.error(e.message || "Could not remove the engine's record. Try again.");
    },
  });

  if (!canDelete) return null;
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" className="px-2" aria-label="More engine actions">
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            className="cursor-pointer text-destructive focus:text-destructive"
            onClick={() => setOpen(true)}
          >
            <Trash2 className="h-4 w-4" />
            Delete engine
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <ConfirmDeleteDialog
        open={open}
        onOpenChange={setOpen}
        title={`Delete «${engine.name}»?`}
        description="egret removes its record; your cluster is not touched."
        confirmText={engine.name}
        onConfirm={() => del.mutate(engine.id)}
      >
        <ul className="list-disc space-y-1 pl-5">
          <li>The bound policy profile is unbound, not deleted.</li>
          <li>Declared catalog attachments are removed; the catalogs themselves are kept.</li>
          <li>The Ranger plugin you installed keeps running until you remove it from your engine.</li>
        </ul>
      </ConfirmDeleteDialog>

      <AlertDialog open={blockedBy !== null} onOpenChange={(o) => !o && setBlockedBy(null)}>
        <AlertDialogContent className="sm:max-w-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>«{engine.name}» is still in use</AlertDialogTitle>
            <AlertDialogDescription>
              Nothing was deleted. Remove or move these first, then delete the engine again.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {blockedBy && <EngineDependentsTable dependents={blockedBy} />}
          <AlertDialogFooter>
            <AlertDialogCancel>Close</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function Overview({
  engine,
  bound,
  profilesQ,
}: {
  engine: ExternalEngine;
  bound: Profile | undefined;
  profilesQ: { isLoading: boolean; isError: boolean };
}) {
  return (
    <>
      <section className="space-y-3 rounded-lg border p-4">
        <p className="text-sm text-muted-foreground">
          External engine: egret holds a record of it and does not monitor it, so there is no health status here.
        </p>
        <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[10rem_1fr]">
          <dt className="text-muted-foreground">Type</dt>
          <dd>{ENGINE_TYPE_LABEL[engine.type] ?? engine.type}</dd>
          <dt className="text-muted-foreground">Address</dt>
          <dd className="font-mono tabular-nums">
            {engine.host}:{engine.port}
          </dd>
          <dt className="text-muted-foreground">Ranger plugin version</dt>
          <dd className="tabular-nums">{engine.rangerPluginVersion}</dd>
          <dt className="text-muted-foreground">Policy profile</dt>
          <dd>
            {bound ? (
              <PolicyProfileLink name={bound.name} />
            ) : profilesQ.isLoading ? (
              "Loading…"
            ) : profilesQ.isError ? (
              "Could not be read"
            ) : (
              <span className="text-muted-foreground">None bound. Bind one on the Settings tab.</span>
            )}
          </dd>
        </dl>
      </section>
      {engine.rangerPluginVersionModelled === false && <UnmodelledVersionNote />}
    </>
  );
}

// ── Declared catalogs ────────────────────────────────────────────────────────

function DeclaredCatalogs({ engine }: { engine: ExternalEngine }) {
  const queryClient = useQueryClient();
  const canUpdate = useCan("catalog_registry:update");
  const [pick, setPick] = useState("");
  const [detachTarget, setDetachTarget] = useState<{ id: string; name: string } | null>(null);

  const registryQ = useControlPlaneCatalogRegistryListRegistryCatalogs<
    controlPlaneCatalogRegistryListRegistryCatalogsResponse,
    ApiError
  >(undefined, { query: { retry: false } });
  const all = registryQ.data?.status === 200 ? registryQ.data.data : [];
  const rows = all.filter((c) => c.engines.some((a) => a.engineId === engine.id));
  const candidates = all.filter((c) => !c.engines.some((a) => a.engineId === engine.id));

  const settle = (catalogId: string) => {
    void queryClient.invalidateQueries({ queryKey: catalogKey(catalogId) });
    void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
    void queryClient.invalidateQueries({
      queryKey: getControlPlaneEnginesRangerPluginConfigQueryKey(engine.id),
    });
  };
  const attach = useControlPlaneCatalogRegistryRegistryAttachCatalog<ApiError>({
    mutation: {
      onSuccess: (_r, v) => {
        settle(v.id);
        setPick("");
        toast.success("Declared attached.");
      },
    },
  });
  const detach = useControlPlaneCatalogRegistryDetachCatalog<ApiError>({
    mutation: {
      onSuccess: (_r, v) => {
        settle(v.id);
        toast.success("Detached.");
      },
    },
  });
  const failure = attach.error ?? detach.error;

  if (registryQ.isError)
    return (
      <Alert variant="destructive">
        <AlertDescription>
          {registryQ.error?.status === 403
            ? "Listing catalogs needs the catalog_registry:read permission, which your account does not have."
            : `Could not read the catalog registry: ${registryQ.error?.message ?? "the request failed"}.`}
        </AlertDescription>
      </Alert>
    );
  if (registryQ.isLoading) return <ListSkeleton rows={3} cols={3} />;

  return (
    <section className="space-y-3">
      <p className="text-sm text-muted-foreground">
        These catalogs are declared as served by this engine, so grants on them reach its policy profile. egret does
        not create these catalogs on your engine — create them there with the same names.
      </p>

      {failure && <ServerRefusal error={failure} testId="declared-error" />}

      {rows.length === 0 ? (
        <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
          No catalogs are declared on this engine yet.
        </p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Catalog</TableHead>
              <TableHead>Kind</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((c) => (
              <TableRow key={c.id}>
                <TableCell className="font-medium">{c.name}</TableCell>
                <TableCell className="text-muted-foreground">{c.kind}</TableCell>
                <TableCell>
                  <Badge variant="outline" title="Recorded by egret; nothing was applied to the engine.">
                    declared
                  </Badge>
                </TableCell>
                <TableCell className="text-right">
                  {canUpdate && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={detach.isPending}
                      onClick={() => setDetachTarget({ id: c.id, name: c.name })}
                    >
                      <Unlink className="h-3.5 w-3.5" />
                      Detach
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}

      {canUpdate ? (
        <div className="flex flex-wrap items-end gap-2">
          <div className="space-y-1.5">
            <Label htmlFor="declare-catalog">Catalog to declare</Label>
            <Select value={pick} onValueChange={setPick}>
              <SelectTrigger id="declare-catalog" className="w-72">
                <SelectValue placeholder={candidates.length ? "Choose a catalog" : "No catalogs left to declare"} />
              </SelectTrigger>
              <SelectContent>
                {candidates.map((c) => (
                  <SelectItem key={c.id} value={c.id}>
                    {c.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button
            disabled={!pick || attach.isPending}
            aria-describedby="declare-why"
            onClick={() => attach.mutate({ id: pick, engineId: engine.id })}
          >
            {attach.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
            Declare attached
          </Button>
          <p id="declare-why" className="text-xs text-muted-foreground">
            {pick ? "" : "Choose a catalog to declare."}
          </p>
        </div>
      ) : (
        <p className="text-xs text-muted-foreground" data-testid="declared-readonly">
          Declaring and detaching catalogs needs the catalog_registry:update permission, which this account does not
          have. A platform administrator can grant it.
        </p>
      )}

      <AlertDialog open={detachTarget !== null} onOpenChange={(o) => !o && setDetachTarget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Detach «{detachTarget?.name}» from «{engine.name}»?</AlertDialogTitle>
            <AlertDialogDescription>
              egret stops treating this engine as serving the catalog and recompiles the bound profile. Nothing changes
              on your cluster, and the catalog itself is kept.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (detachTarget) detach.mutate({ id: detachTarget.id, engineId: engine.id });
                setDetachTarget(null);
              }}
            >
              Detach
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

// ── Settings: the PATCH form ─────────────────────────────────────────────────

function ExternalSettings({
  engine,
  profilesQ,
  bound,
}: {
  engine: ExternalEngine;
  profilesQ: { isLoading: boolean; isError: boolean; error: ApiError | null; data: Profile[] | undefined };
  bound: Profile | undefined;
}) {
  const canUpdate = useCan("engine:update");
  const modelled = useModelledPlugins();
  if (!canUpdate)
    return (
      <Alert data-testid="settings-readonly">
        <AlertDescription>
          Changing the address, plugin version or policy profile of an external engine needs the engine:update
          permission, which this account does not have. A platform administrator can grant it.
        </AlertDescription>
      </Alert>
    );
  // The form seeds its version choice from the modelled list too, so wait for that as well.
  if (profilesQ.isLoading || modelled.loading) return <ListSkeleton rows={4} cols={2} />;
  // The form seeds from the loaded profile list, so it mounts after it settled.
  return <SettingsForm engine={engine} profiles={profilesQ.isError ? null : (profilesQ.data ?? [])} bound={bound} />;
}

function SettingsForm({
  engine,
  profiles,
  bound,
}: {
  engine: ExternalEngine;
  profiles: Profile[] | null;
  bound: Profile | undefined;
}) {
  const queryClient = useQueryClient();
  const currentVersion = engine.rangerPluginVersion ?? "";
  const modelled = useModelledPlugins();
  const offeredVersions = modelled.versionsFor(engine.type);
  const offered = offeredVersions.includes(currentVersion);

  const [host, setHost] = useState(engine.host ?? "");
  const [port, setPort] = useState(String(engine.port ?? ""));
  const [versionChoice, setVersionChoice] = useState(offered ? currentVersion : OTHER);
  const [otherVersion, setOtherVersion] = useState(offered ? "" : currentVersion);
  const [profileId, setProfileId] = useState(bound ? String(bound.id) : "");

  const version = versionChoice === OTHER ? otherVersion.trim() : versionChoice;
  const versionOk = PLUGIN_VERSION_RE.test(version);
  const portNumber = /^[0-9]+$/.test(port) ? Number(port) : NaN;
  const portOk = Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;

  const patch: UpdateExternalEngine = {};
  if (host.trim() !== (engine.host ?? "")) patch.host = host.trim();
  if (portOk && portNumber !== engine.port) patch.port = portNumber;
  if (versionOk && version !== currentVersion) patch.rangerPluginVersion = version;
  if (profiles && profileId !== (bound ? String(bound.id) : ""))
    patch.authzServiceId = profileId === "" ? null : Number(profileId);
  const changed = Object.keys(patch).length > 0;

  const invalid: string[] = [];
  if (!host.trim()) invalid.push("a host");
  if (!portOk) invalid.push("a port from 1 to 65535");
  if (!versionOk) invalid.push("a plugin version like 2.8.0");
  const blockedReason = invalid.length
    ? `Enter ${invalid.join(", ")}.`
    : !changed
      ? "Nothing has changed yet."
      : null;

  useUnsavedChangesWarning(changed);

  const save = useControlPlaneEnginesUpdateExternalEngine<ApiError>({
    mutation: {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: engineDetailKey(engine.id) });
        void queryClient.invalidateQueries({ queryKey: enginesQueryKey });
        void queryClient.invalidateQueries({ queryKey: profilesQueryKey });
        void queryClient.invalidateQueries({
          queryKey: getControlPlaneEnginesRangerPluginConfigQueryKey(engine.id),
        });
        toast.success("Saved.");
      },
    },
  });

  const profilesOfType = (profiles ?? []).filter((p) => p.type === engine.type);

  return (
    <form
      className="max-w-3xl space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!blockedReason) save.mutate({ id: engine.id, data: patch });
      }}
    >
      {save.isError && <ServerRefusal error={save.error} testId="settings-error" />}

      <section className="space-y-3 rounded-lg border p-4" aria-labelledby="es-address-h">
        <h2 id="es-address-h" className="text-base font-semibold">
          Address
        </h2>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="es-host">Host</Label>
            <Input id="es-host" value={host} onChange={(e) => setHost(e.target.value)} autoComplete="off" />
            <p className="text-xs text-muted-foreground">A host name or IP address: no scheme, port or path.</p>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="es-port">Port</Label>
            <Input
              id="es-port"
              type="number"
              min={1}
              max={65535}
              className="tabular-nums"
              value={port}
              onChange={(e) => setPort(e.target.value)}
            />
          </div>
        </div>
      </section>

      <section className="space-y-3 rounded-lg border p-4" aria-labelledby="es-ranger-h">
        <h2 id="es-ranger-h" className="text-base font-semibold">
          Ranger
        </h2>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="es-version">Plugin version</Label>
            <Select value={versionChoice} onValueChange={setVersionChoice}>
              <SelectTrigger id="es-version">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {offeredVersions.map((v) => (
                  <SelectItem key={v} value={v}>
                    {v}
                  </SelectItem>
                ))}
                <SelectItem value={OTHER}>Other…</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {versionChoice === OTHER && (
            <div className="space-y-1.5">
              <Label htmlFor="es-version-other">Other plugin version</Label>
              <Input
                id="es-version-other"
                className="tabular-nums"
                value={otherVersion}
                onChange={(e) => setOtherVersion(e.target.value)}
                aria-invalid={otherVersion !== "" && !versionOk}
              />
              {otherVersion !== "" && !versionOk && (
                <p className="text-xs text-destructive">Write the version as three numbers, like 2.6.0.</p>
              )}
            </div>
          )}
        </div>
        {modelled.failed && <ModelledPluginsUnavailable />}
        {versionOk && modelled.ready && !modelled.isModelled(engine.type, version) && <UnmodelledVersionNote />}
      </section>

      <section className="space-y-3 rounded-lg border p-4" aria-labelledby="es-profile-h">
        <h2 id="es-profile-h" className="text-base font-semibold">
          Policy profile
        </h2>
        {profiles === null ? (
          <Alert variant="destructive">
            <AlertDescription>
              The policy profiles could not be read, so the binding cannot be changed now. Reload the page to try
              again.
            </AlertDescription>
          </Alert>
        ) : (
          <div className="space-y-1.5">
            <Label htmlFor="es-profile">Policy profile</Label>
            <Select
              value={profileId === "" ? NO_PROFILE : profileId}
              onValueChange={(v) => setProfileId(v === NO_PROFILE ? "" : v)}
            >
              <SelectTrigger id="es-profile" className="w-72">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NO_PROFILE}>No profile</SelectItem>
                {profilesOfType.map((p) => {
                  const owner = p.engines.find((e) => e.id !== engine.id);
                  return (
                    <SelectItem key={p.id} value={String(p.id)} disabled={!!owner}>
                      {p.name}
                      {owner ? ` — bound to ${owner.name}` : ""}
                    </SelectItem>
                  );
                })}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Changing or removing the profile recompiles both the old and the new one. If that would leave two mask or
              row-filter rules deciding one column differently, egret refuses and changes nothing.
            </p>
          </div>
        )}
      </section>

      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" disabled={!!blockedReason || save.isPending} aria-describedby="es-why">
          {save.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
          Save changes
        </Button>
        <p id="es-why" className="text-sm text-muted-foreground">
          {blockedReason ?? ""}
        </p>
      </div>
    </form>
  );
}
