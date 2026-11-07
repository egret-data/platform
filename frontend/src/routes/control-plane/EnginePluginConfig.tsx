import { useState } from "react";
import { Check, Copy, Download } from "lucide-react";
import { toast } from "sonner";

import { ApiError } from "@/api/client";
import {
  type controlPlaneEnginesOpaPluginConfigResponse,
  type controlPlaneEnginesRangerPluginConfigResponse,
  useControlPlaneEnginesOpaPluginConfig,
  useControlPlaneEnginesRangerPluginConfig,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type { RangerPluginFileDto } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { downloadTextFile } from "@/lib/downloadTextFile";
import { UnmodelledVersionNote, WarningCallout } from "./externalEngine";

/** The words for each `authMode` the server can report; the credential's password is never sent. */
function authModeLine(authMode: string): string {
  if (authMode === "engine-credential")
    return "The policy download needs this engine's Ranger credential. Its password was shown once, at registration or rotation; the files below carry a placeholder in its place.";
  if (authMode === "anonymous") return "The policy download is anonymous: no credential is needed.";
  return authMode;
}

function FileBlock({ file }: { file: RangerPluginFileDto }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(file.content);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("Could not copy to the clipboard. Select the text and copy it, or use Download.");
    }
  };
  return (
    <div className="space-y-1.5 rounded-lg border p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-mono text-sm">{file.name}</span>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={copy} aria-label={`Copy ${file.name}`}>
            {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
            {copied ? "Copied" : "Copy"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => downloadTextFile(file.name, file.content)}
            aria-label={`Download ${file.name}`}
          >
            <Download className="h-3.5 w-3.5" />
            Download
          </Button>
        </div>
      </div>
      <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">{file.content}</pre>
    </div>
  );
}

/**
 * The Ranger plugin configuration an operator installs on an external engine.
 * Everything shown comes from `GET /engines/{id}/ranger-plugin-config`, and only the
 * documented fields are rendered, so a credential can never reach the screen: the
 * server sends none, and this component reads none.
 */
export function EnginePluginConfig({
  engineId,
  mechanism,
  onOpenSettings,
  onReload,
}: {
  onReload: () => void;
  engineId: string;
  /** The bound profile's mechanism; `opa` shows the OPA plugin's files instead of Ranger's. */
  mechanism?: string;
  onOpenSettings: () => void;
}) {
  return mechanism === "opa" ? (
    <OpaEnginePluginConfig engineId={engineId} onReload={onReload} />
  ) : (
    <RangerEnginePluginConfig engineId={engineId} onOpenSettings={onOpenSettings} onReload={onReload} />
  );
}

/** A 409 from either plugin-config route: the page's profile list and the server disagree (a race, or a switch just made). */
function Disagreement({
  testId,
  children,
  onReload,
  onOpenSettings,
}: {
  testId: string;
  children: string;
  onReload: () => void;
  onOpenSettings?: () => void;
}) {
  return (
    <Alert data-testid={testId}>
      <AlertDescription className="space-y-2">
        <p>{children}</p>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={onReload}>
            Reload
          </Button>
          {onOpenSettings && (
            <Button size="sm" variant="ghost" onClick={onOpenSettings}>
              Open Settings
            </Button>
          )}
        </div>
      </AlertDescription>
    </Alert>
  );
}

function OpaEnginePluginConfig({ engineId, onReload }: { engineId: string; onReload: () => void }) {
  const q = useControlPlaneEnginesOpaPluginConfig<controlPlaneEnginesOpaPluginConfigResponse, ApiError>(engineId, {
    query: { retry: false },
  });
  if (q.isError && q.error?.status === 409)
    return (
      <Disagreement testId="plugin-config-not-opa" onReload={onReload}>
        This engine&apos;s profile is no longer on OPA: its mechanism was changed after this page loaded. Reload to
        see the plugin configuration for the current mechanism.
      </Disagreement>
    );
  if (q.isError)
    return (
      <Alert variant="destructive" data-testid="plugin-config-error">
        <AlertDescription>{q.error?.message ?? "Could not read the plugin configuration."}</AlertDescription>
      </Alert>
    );
  if (q.isLoading || !q.data) return <ListSkeleton rows={4} cols={2} />;
  if (q.data.status !== 200)
    return (
      <Alert variant="destructive">
        <AlertDescription>The plugin configuration could not be read. Reload the page to try again.</AlertDescription>
      </Alert>
    );
  const cfg = q.data.data;
  return (
    <div className="space-y-4" data-testid="opa-plugin-config">
      <section className="space-y-2 rounded-lg border p-4">
        <h2 className="text-base font-semibold">Install on your engine</h2>
        <p className="text-sm text-muted-foreground">
          Install these two files on a Trino 483 or later coordinator with plugin/opa. egret answers the plugin&apos;s
          requests per decision; nothing is downloaded.
        </p>
      </section>

      {cfg.warnings.length > 0 && (
        <div className="space-y-2" data-testid="plugin-config-warnings">
          {cfg.warnings.map((w) => (
            <WarningCallout key={w}>{w}</WarningCallout>
          ))}
        </div>
      )}

      {cfg.adminMustSupply.length > 0 && (
        <section className="space-y-1.5 rounded-lg border p-4">
          <h2 className="text-base font-semibold">You must supply</h2>
          <p className="text-sm text-muted-foreground">egret renders no credential and invents none.</p>
          <ul className="list-disc space-y-1 pl-5 text-sm">
            {cfg.adminMustSupply.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ul>
        </section>
      )}

      <section className="space-y-2">
        <h2 className="text-base font-semibold">Files</h2>
        {cfg.files.map((f) => (
          <FileBlock key={f.name} file={f} />
        ))}
      </section>
    </div>
  );
}

function RangerEnginePluginConfig({ engineId, onOpenSettings, onReload }: { engineId: string; onOpenSettings: () => void; onReload: () => void }) {
  const q = useControlPlaneEnginesRangerPluginConfig<controlPlaneEnginesRangerPluginConfigResponse, ApiError>(
    engineId,
    { query: { retry: false } },
  );

  // FE-STATE-1: error first, so a failed read never renders as "nothing to install".
  // On an external engine the one 409 is "bound to no policy profile": a step the
  // operator has not taken yet, said in a sentence with the way to it (KI-204),
  // not the server's `conflict: …` text.
  if (q.isError && q.error?.status === 409)
    return (
      <Disagreement testId="plugin-config-no-profile" onReload={onReload} onOpenSettings={onOpenSettings}>
        egret reports no policy profile bound to this engine, which differs from this page&apos;s profile list. Reload
        to check, or bind a profile in Settings.
      </Disagreement>
    );
  if (q.isError)
    return (
      <Alert variant="destructive" data-testid="plugin-config-error">
        <AlertDescription>{q.error?.message ?? "Could not read the plugin configuration."}</AlertDescription>
      </Alert>
    );
  if (q.isLoading || !q.data) return <ListSkeleton rows={4} cols={2} />;
  if (q.data.status !== 200)
    return (
      <Alert variant="destructive">
        <AlertDescription>The plugin configuration could not be read. Reload the page to try again.</AlertDescription>
      </Alert>
    );

  const cfg = q.data.data;
  return (
    <div className="space-y-4">
      <section className="space-y-2 rounded-lg border p-4">
        <h2 className="text-base font-semibold">Install on your engine</h2>
        <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[10rem_1fr]">
          <dt className="text-muted-foreground">Ranger service name</dt>
          <dd className="font-mono">{cfg.serviceName}</dd>
          <dt className="text-muted-foreground">Policy download URL</dt>
          <dd className="break-all font-mono">{cfg.downloadUrl}</dd>
          <dt className="text-muted-foreground">Authentication</dt>
          <dd>{authModeLine(cfg.authMode)}</dd>
          <dt className="text-muted-foreground">Declared plugin version</dt>
          <dd className="tabular-nums">{cfg.rangerPluginVersion ?? "not declared"}</dd>
        </dl>
      </section>

      {!cfg.rangerPluginVersionModelled && <UnmodelledVersionNote testId="plugin-config-unmodelled" />}

      {cfg.warnings.length > 0 && (
        <div className="space-y-2" data-testid="plugin-config-warnings">
          {cfg.warnings.map((w) => (
            <WarningCallout key={w}>{w}</WarningCallout>
          ))}
        </div>
      )}

      {cfg.adminMustSupply.length > 0 && (
        <section className="space-y-1.5 rounded-lg border p-4">
          <h2 className="text-base font-semibold">You must supply</h2>
          <p className="text-sm text-muted-foreground">egret renders no credential and invents none.</p>
          <ul className="list-disc space-y-1 pl-5 text-sm">
            {cfg.adminMustSupply.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ul>
        </section>
      )}

      {cfg.assumptions.length > 0 && (
        <section className="space-y-1.5 rounded-lg border p-4">
          <h2 className="text-base font-semibold">What egret assumes</h2>
          <p className="text-sm text-muted-foreground">
            egret cannot check these on an engine it does not run. Explain and the migration check rely on them.
          </p>
          <ul className="list-disc space-y-1 pl-5 text-sm">
            {cfg.assumptions.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ul>
        </section>
      )}

      <section className="space-y-2">
        <h2 className="text-base font-semibold">Files</h2>
        {cfg.files.map((f) => (
          <FileBlock key={f.name} file={f} />
        ))}
      </section>
    </div>
  );
}
