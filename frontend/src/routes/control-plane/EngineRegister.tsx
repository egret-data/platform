import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2 } from "lucide-react";

import { ApiError } from "@/api/client";
import { type Profile, listProfiles } from "@/api/policies";
import {
  type controlPlaneCatalogRegistryListRegistryCatalogsResponse,
  useControlPlaneCatalogRegistryListRegistryCatalogs,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { useControlPlaneEnginesRegisterExternalEngine } from "@/api/generated/control-plane-engines/control-plane-engines";
import type { RangerCredentialDto, RegisterExternalEngine } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { enginesQueryKey } from "./ControlPlaneEngines";
import {
  ENGINE_TYPE_LABEL,
  PLUGIN_VERSION_RE,
  ServerRefusal,
  UnmodelledVersionNote,
  profilesQueryKey,
  useCan,
  ModelledPluginsUnavailable,
  useModelledPlugins,
} from "./externalEngine";
import { OneTimeCredentialDialog } from "./RangerCredential";

const OTHER = "__other__";
const NO_PROFILE = "__none__";

/**
 * Control Plane → Engines → Register external engine.
 *
 * A page, not a dialog (FE-PAGE-3): four sections. It records an engine egret does
 * not run. Nothing is provisioned and nothing is applied to the engine; the result
 * is a row, an optional profile binding and declared catalog attachments, then the
 * Ranger plugin configuration the operator installs.
 *
 * The 201 carries the engine's Ranger credential, shown ONCE (spec A10/A11):
 * it is copied into this page's state, the mutation is reset (and kept by no
 * cache, `gcTime: 0`), and a dialog that only closes once the operator says
 * they saved it shows it. Then the page goes to the engine's Plugin config tab
 * and the password is gone with it.
 */
export default function EngineRegister() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const canCreate = useCan("engine:create");
  const modelled = useModelledPlugins();

  const [name, setName] = useState("");
  const [type, setType] = useState("trino");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [versionChoice, setVersionChoice] = useState("");
  const [otherVersion, setOtherVersion] = useState("");
  const [profileId, setProfileId] = useState("");
  const [catalogIds, setCatalogIds] = useState<string[]>([]);
  const [issued, setIssued] = useState<{ engineId: string; credential: RangerCredentialDto } | null>(null);

  const profilesQ = useQuery<Profile[], ApiError>({
    queryKey: profilesQueryKey,
    queryFn: listProfiles,
    retry: false,
  });
  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs<
    controlPlaneCatalogRegistryListRegistryCatalogsResponse,
    ApiError
  >(undefined, { query: { retry: false } });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];

  const register = useControlPlaneEnginesRegisterExternalEngine<ApiError>({
    mutation: {
      // Not kept once unobserved: the 201 body carries a password.
      gcTime: 0,
      // Navigating to the result after the credential dialog, so no toast (FE-ACTION-2).
      onSuccess: (res) => {
        void queryClient.invalidateQueries({ queryKey: enginesQueryKey });
        void queryClient.invalidateQueries({ queryKey: profilesQueryKey });
        if (res.status === 201) {
          const { username, password, credentialId } = res.data.rangerCredential;
          setIssued({ engineId: res.data.id, credential: { username, password, credentialId } });
        }
        // Drop the response from the hook: from here on, `issued` is its only holder.
        register.reset();
      },
    },
  });

  const version = versionChoice === OTHER ? otherVersion.trim() : versionChoice;
  const portNumber = /^[0-9]+$/.test(port) ? Number(port) : NaN;
  const versionOk = PLUGIN_VERSION_RE.test(version);
  const portOk = Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;

  const missing: string[] = [];
  if (!name.trim()) missing.push("a name");
  if (!host.trim()) missing.push("a host");
  if (!portOk) missing.push("a port");
  if (!versionOk) missing.push("a plugin version");
  const blockedReason =
    missing.length === 0
      ? null
      : `Enter ${missing.length === 1 ? missing[0] : `${missing.slice(0, -1).join(", ")} and ${missing[missing.length - 1]}`} (the version as x.y.z) to register.`;

  const dirty = !!(name || host || port || versionChoice || profileId || catalogIds.length);
  // While the credential is on screen, leaving the page would lose it too.
  useUnsavedChangesWarning((dirty && !issued) || issued !== null);

  const profilesOfType = (profilesQ.data ?? []).filter((p) => p.type === type);
  const chosenProfile = profilesOfType.find((p) => String(p.id) === profileId);

  const changeType = (next: string) => {
    setType(next);
    // A profile of the other type cannot be bound: drop it rather than send a doomed request.
    if (!(profilesQ.data ?? []).some((p) => String(p.id) === profileId && p.type === next)) setProfileId("");
  };

  const submit = () => {
    if (blockedReason) return;
    const body: RegisterExternalEngine = {
      name: name.trim(),
      type,
      host: host.trim(),
      port: portNumber,
      rangerPluginVersion: version,
      catalogs: catalogIds,
    };
    if (chosenProfile) body.authzServiceId = chosenProfile.id;
    register.mutate({ data: body });
  };

  const toggleCatalog = (id: string, on: boolean) =>
    setCatalogIds((cur) => (on ? [...cur, id] : cur.filter((c) => c !== id)));

  return (
    <div className="space-y-4">
      <Link
        to="/control-plane/engines"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Engines
      </Link>

      <div className="max-w-3xl">
        <h1 className="text-2xl font-semibold leading-tight">Register external engine</h1>
        <p className="text-sm text-muted-foreground">
          Record a Trino or StarRocks cluster that you run yourself. egret does not provision, restart or monitor
          it; it holds the record so a policy profile can be bound to the engine and you can read the Ranger plugin
          configuration to install on it.
        </p>
      </div>

      {!canCreate ? (
        <Alert>
          <AlertDescription>
            Registering an engine needs the engine:create permission, which your account does not have. A platform
            administrator can grant it.
          </AlertDescription>
        </Alert>
      ) : (
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          {register.isError && <ServerRefusal error={register.error} testId="er-error" />}

          <section className="space-y-3 rounded-lg border p-4" aria-labelledby="er-engine-h">
            <h2 id="er-engine-h" className="text-base font-semibold">
              Engine
            </h2>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="er-name">Name</Label>
                <Input id="er-name" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" />
                <p className="text-xs text-muted-foreground">
                  Lower-case letters, digits and hyphens, up to 63 characters.
                </p>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="er-type">Engine type</Label>
                <Select value={type} onValueChange={changeType}>
                  <SelectTrigger id="er-type">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {Object.entries(ENGINE_TYPE_LABEL).map(([v, label]) => (
                      <SelectItem key={v} value={v}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="er-host">Host</Label>
                <Input
                  id="er-host"
                  value={host}
                  onChange={(e) => setHost(e.target.value)}
                  autoComplete="off"
                  placeholder="trino.example.com"
                />
                <p className="text-xs text-muted-foreground">A host name or IP address: no scheme, port or path.</p>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="er-port">Port</Label>
                <Input
                  id="er-port"
                  type="number"
                  min={1}
                  max={65535}
                  className="tabular-nums"
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                />
                <p className="text-xs text-muted-foreground">
                  {type === "starrocks" ? "The FE query port (usually 9030)." : "Trino's HTTP port."}
                </p>
              </div>
            </div>
          </section>

          <section className="space-y-3 rounded-lg border p-4" aria-labelledby="er-ranger-h">
            <h2 id="er-ranger-h" className="text-base font-semibold">
              Ranger
            </h2>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="er-version">Plugin version</Label>
                <Select value={versionChoice} onValueChange={setVersionChoice}>
                  <SelectTrigger id="er-version">
                    <SelectValue placeholder="Choose a version" />
                  </SelectTrigger>
                  <SelectContent>
                    {modelled.versionsFor(type).map((v) => (
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
                  <Label htmlFor="er-version-other">Other plugin version</Label>
                  <Input
                    id="er-version-other"
                    value={otherVersion}
                    onChange={(e) => setOtherVersion(e.target.value)}
                    placeholder="2.6.0"
                    className="tabular-nums"
                    aria-invalid={otherVersion !== "" && !versionOk}
                  />
                  {otherVersion !== "" && !versionOk && (
                    <p className="text-xs text-destructive">Write the version as three numbers, like 2.6.0.</p>
                  )}
                </div>
              )}
            </div>
            {modelled.failed ? (
              <ModelledPluginsUnavailable />
            ) : (
              <p className="text-sm text-muted-foreground">{modelled.hint}</p>
            )}
            {versionOk && modelled.ready && !modelled.isModelled(type, version) && <UnmodelledVersionNote />}
          </section>

          <section className="space-y-3 rounded-lg border p-4" aria-labelledby="er-profile-h">
            <h2 id="er-profile-h" className="text-base font-semibold">
              Policy profile
            </h2>
            <p className="text-sm text-muted-foreground">
              Bind a {ENGINE_TYPE_LABEL[type]} profile now, or leave it and bind one later from the engine&apos;s
              Settings tab. A profile can belong to one engine.
            </p>
            {profilesQ.isLoading ? (
              <Skeleton className="h-9 w-72" />
            ) : profilesQ.isError ? (
              <Alert variant="destructive">
                <AlertDescription>
                  Could not read the policy profiles: {profilesQ.error?.message ?? "the request failed"}. You can still
                  register the engine and bind a profile later.
                </AlertDescription>
              </Alert>
            ) : (
              <div className="space-y-1.5">
                <Label htmlFor="er-profile">Policy profile</Label>
                <Select
                  value={profileId === "" ? NO_PROFILE : profileId}
                  onValueChange={(v) => setProfileId(v === NO_PROFILE ? "" : v)}
                >
                  <SelectTrigger id="er-profile" className="w-72">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NO_PROFILE}>No profile yet</SelectItem>
                    {profilesOfType.map((p) => {
                      const owner = p.engines[0];
                      return (
                        <SelectItem key={p.id} value={String(p.id)} disabled={!!owner}>
                          {p.name}
                          {owner ? ` — bound to ${owner.name}` : ""}
                        </SelectItem>
                      );
                    })}
                  </SelectContent>
                </Select>
                {profilesOfType.length === 0 && (
                  <p className="text-xs text-muted-foreground">
                    There is no {ENGINE_TYPE_LABEL[type]} profile yet. Create one under Governance → Policies.
                  </p>
                )}
              </div>
            )}
          </section>

          <section className="space-y-3 rounded-lg border p-4" aria-labelledby="er-catalogs-h">
            <h2 id="er-catalogs-h" className="text-base font-semibold">
              Catalogs
            </h2>
            <p className="text-sm text-muted-foreground">
              Declare which registry catalogs this engine serves, so grants on them reach its profile. egret does not
              create these catalogs on your engine — create them there with the same names.
            </p>
            {catalogsQ.isLoading ? (
              <Skeleton className="h-20 w-full" />
            ) : catalogsQ.isError ? (
              <Alert variant="destructive">
                <AlertDescription>
                  {catalogsQ.error?.status === 403
                    ? "Listing catalogs needs the catalog_registry:read permission, which your account does not have. You can register the engine now and declare catalogs later."
                    : `Could not read the catalog registry: ${catalogsQ.error?.message ?? "the request failed"}. You can declare catalogs later from the engine's Catalogs tab.`}
                </AlertDescription>
              </Alert>
            ) : catalogs.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                There are no catalogs to declare yet. Create one under Control Plane → Catalogs.
              </p>
            ) : (
              <ul className="divide-y rounded-md border" role="list">
                {catalogs.map((c) => (
                  <li key={c.id} className="flex items-center gap-3 p-3">
                    <Checkbox
                      id={`er-cat-${c.id}`}
                      aria-label={`Declare ${c.name} attached`}
                      checked={catalogIds.includes(c.id)}
                      onCheckedChange={(v) => toggleCatalog(c.id, v === true)}
                    />
                    <span className="text-sm font-medium">{c.name}</span>
                    <span className="text-xs text-muted-foreground">{c.kind}</span>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="submit"
              disabled={!!blockedReason || register.isPending || issued !== null}
              aria-describedby="er-submit-why"
            >
              {register.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Register engine
            </Button>
            <Button type="button" variant="outline" asChild>
              <Link to="/control-plane/engines">Cancel</Link>
            </Button>
            <p id="er-submit-why" className="text-sm text-muted-foreground">
              {blockedReason ?? ""}
            </p>
          </div>
        </form>
      )}

      <OneTimeCredentialDialog
        credential={issued?.credential ?? null}
        engineType={type}
        title="Engine registered: save its Ranger credential"
        intro="Copy it now: egret shows this password once. The engine's Ranger plugin presents it to egret to download policies; without it, the download is refused."
        doneLabel="Open plugin configuration"
        onDone={() => {
          const id = issued?.engineId;
          setIssued(null);
          if (id) navigate(`/control-plane/engines/${id}?tab=plugin-config`);
        }}
      />
    </div>
  );
}
