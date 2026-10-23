import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ExternalLink, Plug, Plus, Server } from "lucide-react";

import { type Engine, getEngine, listEngines } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  FilterableTable,
  type Column,
} from "@/components/explore/FilterableTable";
import { EmptyState } from "@/components/explore/EmptyState";
import { ExternalChip, isExternal, useCan } from "./externalEngine";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";
import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";

/**
 * Control Plane → Engines
 *
 * Platform-admin registry of query engines. Trino and StarRocks clusters can
 * both be provisioned on Kubernetes; RisingWave is a placeholder for a later
 * phase. Lists engines per type, shows a live health badge (per-row status
 * poll), and provisions a new engine against a catalog warehouse — the active
 * tab picks the engine kind for the Add dialog.
 */

type EngineType = {
  value: string;
  label: string;
};

const ENGINE_TYPES: EngineType[] = [
  { value: "trino", label: "Trino" },
  { value: "starrocks", label: "StarRocks" },
  { value: "risingwave", label: "RisingWave" },
];

// Trino and StarRocks provisioning are wired up; RisingWave is not yet.
const PROVISIONABLE = new Set(["trino", "starrocks"]);

// The images the server pins for a new engine whose create body OMITS them
// (`egret_perm::DEFAULT_ENGINE_IMAGES`). DISPLAY ONLY — placeholders and
// "Default:" hints. The create form never sends them: an untouched image field
// is omitted so the server's own pin applies, and the UI cannot drift it onto a
// floating or stale tag. `crates/egret-engine-mgmt/tests/no_floating_images.rs`
// fails when these stop matching the server's pins.
export const DEFAULT_TRINO_IMAGE_VERSION =
  "483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534";
export const DEFAULT_SR_FE_IMAGE =
  "starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31";
export const DEFAULT_SR_CN_IMAGE =
  "starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b";

// Harmless defaults for the still-required Trino-only body fields
// (coordinatorReplicas/workerReplicas/cpuMin/cpuMax/memory) when creating a
// StarRocks engine — the backend ignores them for `kind: "starrocks"` but the
// body validates them as required regardless of kind. Mirrors the Trino
// form's own defaults below.
export const TRINO_FIELD_DEFAULTS = {
  coordinatorReplicas: 1,
  workerReplicas: 2,
  cpuMin: "1",
  cpuMax: "2",
  memory: "4Gi",
};

/**
 * The `host:port` clients reach the engine on, as the server reports it (what
 * a workspace connection to it dials). Rebuilding it here named a host that
 * does not resolve (KI-199).
 */
function engineEndpoint(engine: Engine): string {
  if (engine.endpoint) return engine.endpoint;
  return isExternal(engine) ? `${engine.host}:${engine.port}` : "—";
}

// StarRocks per-user delegation mode (Access parity slice 3 + default-on
// delegation, Task 4). `""` (Auto) is the DEFAULT — omitted from the body so
// the backend resolves jwt→impersonate gracefully. `"jwt"`/`"impersonate"`
// mirror Trino's two delegation modes (see `EngineSecurity.tsx`'s
// `DelegationMode`), applied to the FE JWT integration / service user the
// backend provisions (`create_starrocks_engine`). `"none"` is an EXPLICIT
// open-engine opt-out — sent literally, never omitted.
type StarRocksDelegationOption = "" | "jwt" | "impersonate" | "none";

export const DELEGATION_HELP: Record<StarRocksDelegationOption, string> = {
  "": "Recommended — per-user JWT when the platform supports it, otherwise impersonation.",
  jwt: "Each query uses the caller's own short-lived JWT, bound to their identity over TLS. No shared credential. OIDC users only.",
  impersonate:
    "egret connects as a trusted service account and runs the query as the caller. Works for any signed-in user; runs over TLS.",
  none: "No delegation — the engine's own configured credential is used for all queries.",
};

export const enginesQueryKey = ["cp", "engines"] as const;

export default function ControlPlaneEngines() {
  const engines = useQuery<Engine[], ApiError>({
    queryKey: enginesQueryKey,
    queryFn: listEngines,
    retry: false,
  });

  const navigate = useNavigate();
  const [activeType, setActiveType] = useState(ENGINE_TYPES[0].value);

  const canProvision = PROVISIONABLE.has(activeType);
  const goCreate = (type: string) =>
    navigate(`/control-plane/engines/new?type=${type}`);

  const canRegister = useCan("engine:create");

  const all = engines.data ?? [];
  const total = all.length;
  // An external engine's stored status is a placeholder egret never reads, so it
  // counts as neither running nor not running; it has its own figure.
  const external = all.filter(isExternal).length;
  const managed = all.filter((e) => !isExternal(e));
  const running = managed.filter((e) => (e.status ?? "").toLowerCase() === "running").length;

  return (
    <PageContainer>
      <div className="space-y-4" data-testid="engines-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <PageTitle>Engines</PageTitle>
          <p className="text-sm text-muted-foreground">
            Platform query engines. Provision a Trino or StarRocks cluster against a catalog
            warehouse and monitor its health. RisingWave arrives in a later phase.
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {canRegister && (
            <Button variant="outline" asChild>
              <Link to="/control-plane/engines/register">
                <Plug className="h-4 w-4" />
                Register external engine
              </Link>
            </Button>
          )}
          <Button
            onClick={() => goCreate(activeType)}
            disabled={!canProvision}
            title={canProvision ? undefined : "Coming soon"}
          >
            <Plus className="h-4 w-4" />
            Add engine
          </Button>
        </div>
      </div>

      <SummaryStrip
        items={[
          { label: "engines", value: total },
          { label: "running", value: running },
          { label: "not running", value: managed.length - running },
          ...(external > 0 ? [{ label: "external", value: external }] : []),
        ]}
      />

      {engines.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load engines. Please try again.</AlertDescription>
        </Alert>
      ) : (
        <Tabs value={activeType} onValueChange={setActiveType}>
          <TabsList>
            {ENGINE_TYPES.map((type) => (
              <TabsTrigger key={type.value} value={type.value}>
                {type.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {ENGINE_TYPES.map((type) => (
            <TabsContent key={type.value} value={type.value} className="mt-4">
              <EngineTable
                label={type.label}
                rows={(engines.data ?? []).filter((e) => e.type === type.value)}
                loading={engines.isLoading}
                onAdd={
                  PROVISIONABLE.has(type.value)
                    ? () => goCreate(type.value)
                    : undefined
                }
              />
            </TabsContent>
          ))}
        </Tabs>
      )}
      </div>
    </PageContainer>
  );
}

// Paginate long engine lists: the platform can hold hundreds of engines, and
// each rendered row mounts a live status poll (`EngineStatusBadge`), so
// capping the visible rows also caps the polling fan-out.
const ENGINES_PAGE_SIZE = 25;

function EngineTable({
  label,
  rows,
  loading,
  onAdd,
}: {
  label: string;
  rows: Engine[];
  loading: boolean;
  onAdd?: () => void;
}) {
  if (loading) return <ListSkeleton rows={6} cols={5} />;

  const columns: Column<Engine>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (e) => e.name,
      cell: (e) => (
        <Link
          to={`/control-plane/engines/${e.id}`}
          className={ENTITY_LINK_CLASS}
        >
          {e.name}
        </Link>
      ),
    },
    {
      key: "type",
      label: "Type",
      value: (e) => e.type,
      cell: (e) => <Badge variant="secondary">{e.type}</Badge>,
    },
    {
      key: "endpoint",
      label: "Endpoint",
      value: (e) => engineEndpoint(e),
      cell: (e) => (
        <span
          className="block max-w-[24rem] truncate font-mono text-xs text-muted-foreground"
          title={engineEndpoint(e)}
        >
          {engineEndpoint(e)}
        </span>
      ),
    },
    {
      key: "status",
      label: "Status",
      // An external engine is not monitored: a chip stands where the status would be.
      cell: (e) => (isExternal(e) ? <ExternalChip /> : <EngineStatusBadge id={e.id} fallback={e.status} />),
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (e) => (
        <Button variant="ghost" size="sm" asChild>
          <Link
            to={`/control-plane/engines/${e.id}`}
            aria-label={`View ${e.name}`}
          >
            <ExternalLink className="h-4 w-4" />
            Details
          </Link>
        </Button>
      ),
    },
  ];

  return (
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(e) => e.id}
      searchPlaceholder={`Search ${label} engines…`}
      initialSort={{ key: "name", dir: "asc" }}
      pageSize={ENGINES_PAGE_SIZE}
      emptyText={
        <EmptyState
          icon={Server}
          title={`No ${label} engines yet`}
          // No CTA button here: the page header already shows a persistent
          // "Add engine" action, so a second one would be redundant (and the
          // wording adapts to whether this engine kind is provisionable yet).
          description={
            onAdd
              ? 'Use "Add engine" above to provision a cluster against a catalog warehouse.'
              : "Support for this engine arrives in a later phase."
          }
        />
      }
    />
  );
}


/**
 * A live Kubernetes phase OR — when the per-row poll has not answered — the
 * stored `engines.status`, which is why `deleting` is in the in-flight arm.
 * `deleting` is one of that column's four values
 * (`egret-engine-mgmt/src/store.rs`); it used to fall through to `neutral`
 * while `EnginesTab`'s `EngineHealthBadge`, which reads the stored value
 * directly, painted it `highlight`, so the two pages showed the same word in two
 * colours and a comment over there claimed they could not (whole-branch
 * review, F6).
 */
function phaseVariant(phase: string): StatusTone {
  switch (phase.toLowerCase()) {
    case "running":
    case "ready":
      return "success";
    case "failed":
    case "error":
      return "destructive";
    case "provisioning":
    case "pending":
    case "creating":
    case "updating":
    case "deleting":
      return "highlight";
    default:
      return "neutral";
  }
}

/** Per-row live health: polls `getEngine` and paints `status.phase` as a badge. */
function EngineStatusBadge({ id, fallback }: { id: string; fallback: string }) {
  const detail = useQuery({
    queryKey: ["cp", "engine", id],
    queryFn: () => getEngine(id),
    refetchInterval: 5000,
    retry: false,
  });

  const phase = detail.data?.status.phase ?? fallback;
  return <StatusBadge tone={phaseVariant(phase)}>{phase}</StatusBadge>;
}
