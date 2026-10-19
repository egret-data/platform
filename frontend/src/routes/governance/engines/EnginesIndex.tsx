import { useState, type FormEvent, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { EngineDto, EnginePluginResponse, KindStateDto, ProfileDto } from "@/api/generated/model";
import { useControlPlaneEnginesListEngines } from "@/api/generated/control-plane-engines/control-plane-engines";
import { KIND_STATE_WORD } from "@/routes/governance/engines/tabs/PluginTab";
import { getControlPlaneGovernanceEnginePluginQueryOptions } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermListAllEngineGrants } from "@/api/generated/perm/perm";
import {
  getPolicyAdminListProfilesQueryKey,
  usePolicyAdminCreateProfile,
  usePolicyAdminDeleteProfile,
  usePolicyAdminListProfiles,
} from "@/api/generated/policy-admin/policy-admin";
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section, TabError } from "@/routes/governance/tabParts";
import { MissingProfileNotice } from "@/routes/governance/engines/MissingProfileNotice";
import { OriginBadge, TypeBadge } from "@/routes/governance/engines/PageFrame";
import { engineUrl, profileUrl } from "@/routes/governance/engines/engineUrl";

export const NO_ENGINE_RECORDED = "No engine is recorded. Engines are created in Control plane.";
export const EVERY_PROFILE_BOUND = "Every profile is bound to an engine.";

/** One row of the Engines table, from Control plane's engine list or, without engine:read, a profile's binding. */
interface EngineRow {
  id: string;
  name: string;
  type: string;
  origin: string;
  /** The bound profile; `undefined` while the profile list is unknown (loading or failed). */
  profile: ProfileDto | null | undefined;
}

/**
 * Worst first: the index shows one word per engine, the worst of its kinds
 * (spec §5). Which kinds an engine is expected to download is the server's
 * judgement (`judge_plugins`, fix D1): a kind it does not expect is
 * `not_expected` and ranks last, so the index and the Plugin tab agree.
 */
const STATE_ORDER: KindStateDto[] = ["silent", "behind", "quiet", "not_reported", "pending", "up_to_date", "not_expected"];
const NOT_JUDGED_WORD: Record<string, string> = {
  external: "Not judged: external",
  not_running: "Not judged: not running",
  no_profile: "Not judged: no profile",
};

/** The plugin column's word for one engine's plugin route answer. */
export function pluginWord(p: EnginePluginResponse): string {
  if (p.judged.state === "not_judged") return NOT_JUDGED_WORD[p.judged.reason] ?? "Not judged";
  const worst = STATE_ORDER.find((s) => p.kinds.some((k) => k.state === s));
  return worst ? KIND_STATE_WORD[worst] : KIND_STATE_WORD.not_reported;
}

/**
 * Profiles that no engine downloads (spec §5): no bound engine, and not named
 * as another profile's tag service (a Trino profile's tag policies live in its
 * tag service, which has no engine of its own; spec premise 6).
 */
export function unboundProfiles(profiles: ProfileDto[]): ProfileDto[] {
  const tagServices = new Set(profiles.map((p) => p.tag_service).filter((t): t is string => !!t));
  return profiles.filter((p) => p.engines.length === 0 && !tagServices.has(p.name));
}

/**
 * Hand-written policies over every profile, each policy once. A profile's
 * `policy_counts` is its own plus its tag service's (Q1), and two profiles may
 * name one tag service: `config.tagService` is free JSON with no UNIQUE, and
 * migration 0182 bumps every profile linking one. Summing the profiles would
 * count a shared tag service once per linker, so each profile adds only its
 * own: its count less its tag service's own (exact along any chain; a cycle,
 * which no writer produces, stops at the profile it started from).
 */
export function handWrittenTotal(profiles: ProfileDto[]): number {
  const byName = new Map(profiles.map((p) => [p.name, p]));
  const own = (p: ProfileDto, seen: Set<string>): number => {
    const tag = p.tag_service ? byName.get(p.tag_service) : undefined;
    if (!tag || seen.has(tag.name)) return p.policy_counts.operator;
    return p.policy_counts.operator - own(tag, new Set(seen).add(p.name));
  };
  return profiles.reduce((n, p) => n + own(p, new Set([p.name])), 0);
}

/**
 * Governance → Engine & policy (spec §5, EP3): every engine with its profile,
 * plugin state and engine grants, then the profiles no engine downloads.
 * `GET /policies/services` (`policy:read`) carries each bound engine's type and
 * origin and each profile's hand-written count (Q1), so a reader without
 * `engine:read` sees the bound engines; with it, Control plane's list adds the
 * unbound ones and the plugin route its state (one call per engine). Each
 * section loads and fails on its own (FE-STATE-1).
 */
export default function EnginesIndex() {
  const can = usePolicyPermissions();
  const engineRead = can("engine:read");
  const profilesQ = usePolicyAdminListProfiles();
  const enginesQ = useControlPlaneEnginesListEngines({ query: { enabled: engineRead } });
  const grantsQ = usePermListAllEngineGrants();
  const [creating, setCreating] = useState(false);

  const profiles = profilesQ.data?.status === 200 ? profilesQ.data.data : null;
  const cpEngines: EngineDto[] | null = enginesQ.data?.status === 200 ? enginesQ.data.data : null;
  const allGrants = grantsQ.data?.status === 200 ? grantsQ.data.data : null;

  const profileOf = (engineId: string): ProfileDto | null | undefined =>
    profiles ? (profiles.find((p) => p.engines.some((e) => e.id === engineId)) ?? null) : undefined;

  let engineRows: EngineRow[] | null = null;
  if (engineRead) {
    engineRows = cpEngines
      ? cpEngines.map((e) => ({ id: e.id, name: e.name, type: e.type, origin: e.origin, profile: profileOf(e.id) }))
      : null;
  } else if (profiles) {
    engineRows = profiles.flatMap((p) =>
      p.engines.map((e) => ({ id: e.id, name: e.name, type: e.type, origin: e.origin, profile: p })),
    );
  }

  const plugins = useQueries({
    queries: (engineRead && cpEngines ? cpEngines : []).map((e) =>
      getControlPlaneGovernanceEnginePluginQueryOptions(e.id),
    ),
  });
  const pluginById = new Map<string, (typeof plugins)[number]>();
  (engineRead && cpEngines ? cpEngines : []).forEach((e, i) => pluginById.set(e.id, plugins[i]));

  const orphans = profiles ? unboundProfiles(profiles) : null;
  const handWritten = profiles ? handWrittenTotal(profiles) : null;

  const grantCount = (id: string): ReactNode =>
    allGrants ? allGrants.filter((g) => g.engineId === id).length : grantsQ.isError ? "Not loaded" : "…";

  const columns: Column<EngineRow>[] = [
    {
      key: "name",
      label: "Engine",
      sortable: true,
      value: (r) => r.name,
      cell: (r) => (
        <Link to={engineUrl(r.id)} className="font-medium text-primary hover:underline">
          {r.name}
        </Link>
      ),
    },
    { key: "type", label: "Type", sortable: true, value: (r) => r.type, cell: (r) => <TypeBadge type={r.type} /> },
    {
      key: "origin",
      label: "Origin",
      sortable: true,
      value: (r) => r.origin,
      cell: (r) => <OriginBadge origin={r.origin} />,
    },
    {
      key: "profile",
      label: "Profile",
      sortable: true,
      value: (r) => r.profile?.name ?? "",
      cell: (r) =>
        r.profile === undefined ? (
          <span className="text-muted-foreground">{profilesQ.isError ? "Not loaded" : "…"}</span>
        ) : r.profile ? (
          <span>{r.profile.name}</span>
        ) : (
          <span className="flex flex-wrap items-center gap-x-2">
            <span className="text-muted-foreground">No profile</span>
            {engineRead ? (
              <Link
                to={`/control-plane/engines/${encodeURIComponent(r.id)}?tab=settings`}
                className="text-sm text-primary hover:underline"
              >
                Bind a profile in Control plane
              </Link>
            ) : null}
          </span>
        ),
    },
    ...(engineRead
      ? [
          {
            key: "plugin",
            label: "Plugin",
            cell: (r: EngineRow) => {
              const q = pluginById.get(r.id);
              if (!q || q.isLoading) return <span className="text-muted-foreground">…</span>;
              if (q.isError || q.data?.status !== 200) return <span className="text-muted-foreground">Not loaded</span>;
              return <span>{pluginWord(q.data.data)}</span>;
            },
          } satisfies Column<EngineRow>,
        ]
      : []),
    {
      key: "grants",
      label: "Engine grants",
      className: "tabular-nums",
      cell: (r) => grantCount(r.id),
    },
    {
      key: "handwritten",
      label: "Hand-written policies",
      className: "tabular-nums",
      cell: (r) =>
        r.profile === undefined ? "…" : r.profile ? r.profile.policy_counts.operator : <span aria-hidden>—</span>,
    },
  ];

  let enginesBody: ReactNode;
  if (engineRead ? enginesQ.isError : profilesQ.isError) {
    const q = engineRead ? enginesQ : profilesQ;
    enginesBody = <TabError what="list of engines" onRetry={() => void q.refetch()} />;
  } else if (!engineRows) {
    enginesBody = <ListSkeleton rows={4} cols={6} />;
  } else if (engineRows.length === 0) {
    enginesBody = <EmptyState title={NO_ENGINE_RECORDED} />;
  } else {
    enginesBody = (
      <FilterableTable
        rows={engineRows}
        columns={columns}
        getId={(r) => r.id}
        searchPlaceholder="Search engines…"
        initialSort={{ key: "name", dir: "asc" }}
        bordered
        noMatchText="No engine matches this search."
      />
    );
  }

  let orphansBody: ReactNode;
  if (profilesQ.isError) {
    orphansBody = <TabError what="list of policy profiles" onRetry={() => void profilesQ.refetch()} />;
  } else if (!orphans) {
    orphansBody = <ListSkeleton rows={3} cols={5} />;
  } else if (orphans.length === 0) {
    orphansBody = <EmptyState title={EVERY_PROFILE_BOUND} />;
  } else {
    orphansBody = <OrphanProfiles rows={orphans} canDelete={can("policy:delete")} />;
  }

  const strip =
    engineRows && orphans
      ? [
          { label: "engines", singular: "engine", value: engineRows.length },
          { label: "profiles without an engine", singular: "profile without an engine", value: orphans.length },
          ...(handWritten !== null
            ? [{ label: "hand-written policies", singular: "hand-written policy", value: handWritten }]
            : []),
        ]
      : null;

  return (
    <ListPageShell
      title="Engine & policy"
      description="Each engine downloads one policy profile. Open an engine to grant engine permissions and see its policies."
      action={
        can("policy:create") ? (
          <Button size="sm" variant="outline" onClick={() => setCreating(true)}>
            <Plus className="mr-1 h-4 w-4" aria-hidden />
            New profile
          </Button>
        ) : null
      }
    >
      <div className="space-y-6">
        <MissingProfileNotice />
        {strip ? <SummaryStrip items={strip} /> : null}
        <Section title="Engines">{enginesBody}</Section>
        <Section title="Profiles without an engine">{orphansBody}</Section>
      </div>
      <NewProfileDialog open={creating} onOpenChange={setCreating} />
    </ListPageShell>
  );
}

function OrphanProfiles({ rows, canDelete }: { rows: ProfileDto[]; canDelete: boolean }) {
  const qc = useQueryClient();
  const [target, setTarget] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const del = usePolicyAdminDeleteProfile();

  const remove = (service: string) =>
    del.mutate(
      { service },
      {
        onSuccess: () => {
          setError(null);
          void qc.invalidateQueries({ queryKey: getPolicyAdminListProfilesQueryKey() });
          toast.success(`Profile ${service} deleted.`);
        },
        onError: (e) => setError(refusalMessage(e, "policy:delete", "The profile could not be deleted.")),
      },
    );

  const columns: Column<ProfileDto>[] = [
    {
      key: "name",
      label: "Profile",
      sortable: true,
      value: (p) => p.name,
      cell: (p) => (
        <Link to={profileUrl(p.name)} className="font-medium text-primary hover:underline">
          {p.name}
        </Link>
      ),
    },
    { key: "type", label: "Type", sortable: true, value: (p) => p.type, cell: (p) => <TypeBadge type={p.type} /> },
    { key: "origin", label: "Origin", value: (p) => p.origin, cell: (p) => <OriginBadge origin={p.origin} /> },
    {
      key: "policies",
      label: "Policies",
      sortable: true,
      className: "tabular-nums",
      value: (p) => p.policy_count,
      cell: (p) => p.policy_count,
    },
    ...(canDelete
      ? [
          {
            key: "actions",
            label: "",
            className: "text-right",
            cell: (p: ProfileDto) => (
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Delete ${p.name}`}
                onClick={() => {
                  setError(null);
                  setTarget(p.name);
                }}
              >
                <Trash2 className="h-4 w-4" aria-hidden />
              </Button>
            ),
          } satisfies Column<ProfileDto>,
        ]
      : []),
  ];

  return (
    <div className="space-y-3">
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <FilterableTable
        rows={rows}
        columns={columns}
        getId={(p) => String(p.id)}
        searchPlaceholder="Search profiles…"
        initialSort={{ key: "name", dir: "asc" }}
        bordered
        noMatchText="No profile matches this search."
      />
      <ConfirmDeleteDialog
        open={target !== null}
        onOpenChange={(o) => {
          if (!o) setTarget(null);
        }}
        title={target ? `Delete profile ${target}?` : "Delete profile?"}
        description="The profile and every policy in it are removed. No engine downloads it."
        confirmText={target ?? ""}
        onConfirm={() => {
          if (target) remove(target);
          setTarget(null);
        }}
      />
    </div>
  );
}

/**
 * New profile (moved from the old Policies page): a profile for an engine egret
 * does not provision, or one to bind later. One dialog with two fields
 * (FE-PAGE-3); a refusal stays in the open dialog with every input kept
 * (FE-ACTION-3).
 */
function NewProfileDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const qc = useQueryClient();
  const create = usePolicyAdminCreateProfile();
  const [name, setName] = useState("");
  const [type, setType] = useState<"trino" | "starrocks">("trino");
  const [error, setError] = useState<string | null>(null);

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setType("trino");
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    const trimmed = name.trim();
    create.mutate(
      { data: { name: trimmed, type } },
      {
        onSuccess: () => {
          void qc.invalidateQueries({ queryKey: getPolicyAdminListProfilesQueryKey() });
          toast.success(`Profile ${trimmed} created.`);
          close(false);
        },
        onError: (err) => setError(refusalMessage(err, "policy:create", "The profile could not be created.")),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent>
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>New profile</DialogTitle>
            <DialogDescription>
              A policy profile for an engine egret does not provision. Provisioning an engine creates its profile.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="new-profile-name">Name</Label>
              <Input
                id="new-profile-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. starrocks-prod"
                required
                autoFocus
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-profile-type">Type</Label>
              <Select value={type} onValueChange={(v) => setType(v as "trino" | "starrocks")}>
                <SelectTrigger id="new-profile-type" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="trino">Trino</SelectItem>
                  <SelectItem value="starrocks">StarRocks</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {error ? (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || !name.trim()}>
              {create.isPending ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : null}
              Create profile
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
