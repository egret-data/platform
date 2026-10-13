/**
 * What the pages for an EXTERNAL engine (SP4d-4) share: the plugin versions egret
 * knows, the permission check, and the two error renderings the server's
 * refusals need.
 *
 * An external engine is a row egret holds for a cluster it never provisions,
 * updates, restarts or deletes. It exists so a policy profile can be bound to it,
 * catalogs can be DECLARED attached, and an operator can read the Ranger plugin
 * configuration to install. Nothing here talks to a cluster.
 */
import { Link } from "react-router-dom";
import { TriangleAlert } from "lucide-react";

import { ApiError } from "@/api/client";
import {
  type controlPlaneEnginesListModelledRangerPluginsResponse,
  useControlPlaneEnginesListModelledRangerPlugins,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type { ModelledRangerPluginDto } from "@/api/generated/model";
import { useAuth } from "@/auth/useAuth";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";

/** The sentence for an engine whose (type, version) pair egret does not model. */
export const UNMODELLED_SENTENCE =
  "egret does not model this engine type and plugin version, so migrations into the bound profile will be refused and Explain will not answer for this engine.";

export const PLUGIN_VERSION_RE = /^[0-9]+\.[0-9]+\.[0-9]+$/;

/**
 * The (type, version) pairs egret's evaluator has been read against, as the server
 * lists them (`GET /engines/modelled-ranger-plugins`). `ready` is false until the
 * list has arrived (or if it failed): callers then offer only "Other…" and show no
 * unmodelled warning, rather than warning on a list they do not have. `failed`
 * is true when the read errored; callers render `ModelledPluginsUnavailable` so
 * the missing choices are explained, never shown as an empty list (FE-STATE-1).
 */
export function useModelledPlugins() {
  const q = useControlPlaneEnginesListModelledRangerPlugins<
    controlPlaneEnginesListModelledRangerPluginsResponse,
    ApiError
  // `retryOnMount: false`: a failed read must stay failed while the page is
  // open. The Settings tab shows a skeleton while this loads and then mounts a
  // form that reads it again; with a refetch on that mount, the errored query
  // went back to pending, the form unmounted, and the two looped forever.
  >({ query: { retry: false, retryOnMount: false, staleTime: 5 * 60_000 } });
  const pairs: ModelledRangerPluginDto[] = q.data?.status === 200 ? q.data.data : [];
  const ready = q.data?.status === 200;
  return {
    ready,
    loading: q.isLoading,
    failed: q.isError,
    versionsFor: (type: string) => pairs.filter((p) => p.type === type).map((p) => p.version),
    isModelled: (type: string, version: string) =>
      pairs.some((p) => p.type === type && p.version === version),
    hint: modelledVersionHint(pairs),
  };
}

function modelledVersionHint(pairs: ModelledRangerPluginDto[]): string {
  const parts = Object.entries(ENGINE_TYPE_LABEL)
    .map(([type, label]) => {
      const versions = pairs.filter((p) => p.type === type).map((p) => p.version);
      return versions.length ? `${label} with plugin ${versions.join(" or ")}` : null;
    })
    .filter((x): x is string => x !== null);
  const only = parts.length ? parts.join(" and ") : "the versions listed here";
  return `egret proves migrations and explains access only for ${only}. Any other version is recorded as you declare it, and those features refuse on this engine.`;
}

/** The line shown when the modelled versions could not be read. */
export function ModelledPluginsUnavailable() {
  return (
    <p className="text-sm text-muted-foreground" data-testid="modelled-unavailable">
      The modelled plugin versions could not be loaded, so only “Other…” is offered. The server checks the
      version you declare when you save.
    </p>
  );
}

export const ENGINE_TYPE_LABEL: Record<string, string> = { trino: "Trino", starrocks: "StarRocks" };

/** The query key family of the profile list, shared with the governance pages. */
export const profilesQueryKey = ["control-plane", "profiles"] as const;

/** platform_admin, or the permission key — the rule `RequirePermission` applies. */
export function useCan(perm: string): boolean {
  const { me } = useAuth();
  return !!me && (me.platform_roles.includes("platform_admin") || me.permissions.includes(perm));
}

export function isExternal(e: { origin?: string | null } | undefined): boolean {
  return e?.origin === "external";
}

/** The chip that stands where a status dot would: egret does not monitor an external engine. */
export function ExternalChip() {
  return (
    <Badge
      variant="outline"
      title="egret holds a record for this engine and does not monitor or manage its cluster."
    >
      External
    </Badge>
  );
}

interface OverlapRule {
  id?: number | null;
  name: string;
}

interface OverlapBody {
  rules: OverlapRule[];
  coordinate?: string;
  kind?: string;
}

/** The structured part of a `rule_overlap` 409, or null for any other error. */
export function overlapFrom(e: unknown): OverlapBody | null {
  if (!(e instanceof ApiError) || e.code !== "rule_overlap") return null;
  const b = e.body as Partial<OverlapBody> | undefined;
  if (!b || !Array.isArray(b.rules)) return null;
  return { rules: b.rules, coordinate: b.coordinate, kind: b.kind };
}

const KIND_LABEL: Record<string, string> = { mask: "mask", row_filter: "row filter" };

/**
 * A refused write, in the server's words. An order-ambiguous mask or row-filter
 * pair (`rule_overlap`) also names the two rules and the column they decide, so
 * the operator knows which to change; any other error is its sentence alone.
 */
export function ServerRefusal({ error, testId }: { error: unknown; testId?: string }) {
  const message =
    error instanceof Error && error.message
      ? error.message
      : "The request failed. Try again, or reload the page if it keeps failing.";
  const overlap = overlapFrom(error);
  return (
    <Alert variant="destructive" data-testid={testId}>
      <AlertDescription className="space-y-2">
        <p>{message}</p>
        {overlap && (
          <div className="space-y-1">
            <p className="text-sm font-medium">
              The two {KIND_LABEL[overlap.kind ?? ""] ?? "policy"} rules that clash
              {overlap.coordinate ? (
                <>
                  {" "}
                  on <span className="font-mono">{overlap.coordinate}</span>
                </>
              ) : null}
              :
            </p>
            <ul className="list-disc space-y-0.5 pl-5">
              {overlap.rules.map((r, i) => (
                <li key={`${r.id ?? "hand"}-${i}`}>
                  <span className="font-mono">{r.name}</span>
                  <span className="text-muted-foreground">
                    {r.id != null ? ` (compiled rule ${r.id})` : " (hand-written policy)"}
                  </span>
                </li>
              ))}
            </ul>
            <p className="text-sm">Nothing was changed. Resolve one of the two, then try again.</p>
          </div>
        )}
      </AlertDescription>
    </Alert>
  );
}

/** A warning that does not rely on colour: an icon and the word "Warning". */
export function WarningCallout({ children, testId }: { children: React.ReactNode; testId?: string }) {
  return (
    <Alert role="note" data-testid={testId}>
      <TriangleAlert className="h-4 w-4" aria-hidden />
      <AlertDescription>
        <span className="font-semibold">Warning: </span>
        {children}
      </AlertDescription>
    </Alert>
  );
}

/** When the (type, version) pair is not modelled, say plainly what that costs. */
export function UnmodelledVersionNote({ testId = "unmodelled-note" }: { testId?: string }) {
  return <WarningCallout testId={testId}>{UNMODELLED_SENTENCE}</WarningCallout>;
}

export function PolicyProfileLink({ name }: { name: string }) {
  return (
    <Link to={`/governance/policies/${encodeURIComponent(name)}`} className="text-primary hover:underline">
      {name}
    </Link>
  );
}
