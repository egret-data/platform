import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ArrowLeft, CheckCircle2, Cog, FileText, Loader2, Lock, Server, Undo2, User } from "lucide-react";
import {
  type ClassifiedPolicy,
  type GatedProfile,
  type MigrateResponse,
  type Profile,
  type UndoResponse,
  type WireDisagreement,
  classifyQueryKey,
  classifyServicePolicies,
  listProfiles,
  migrateServicePolicies,
  undoServicePolicyMigration,
} from "@/api/policies";
import type { ApiError } from "@/api/client";
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { refusalMessage, usePolicyPermissions } from "./policyWrites";

/**
 * Governance → Policies → a profile → Migrate policies (SP4d-3).
 *
 * What each policy of one profile IS (its owner), whether it can be migrated
 * into the permission model and, if not, why — then a proof (a dry-run
 * migration) before the real one, and an undo for what was migrated.
 *
 * **Every sentence that explains a verdict is the server's.** Blockers, the
 * migrated-original sentence, disagreements, the gate's refusal and the
 * notices are rendered verbatim: a second copy of the classifier's grammar in
 * TypeScript would drift from the first, and the operator would be told one
 * thing while the server enforces another.
 *
 * **Migrate is enabled only by a proved, non-stale preview of the CURRENT
 * selection** — `GrantDialog`'s `preview` / `previewError` / `previewedFor` /
 * `previewStale` pattern. Staleness compares the selection itself (the sorted
 * names), so changing it disables Migrate and going back to exactly the
 * previewed set re-enables it, because that preview describes it again.
 */
export default function PolicyMigrationPage() {
  const { service = "" } = useParams();
  const queryClient = useQueryClient();
  const can = usePolicyPermissions();
  // Preview, Migrate and Undo are all `policy:update` on the server (a dry run
  // takes the same profile locks); a reader is not offered any of them.
  const canWrite = can("policy:update");

  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const profile = profiles.data?.find((p) => p.name === service);
  const serviceId = profile?.id;

  const classified = useQuery<ClassifiedPolicy[], ApiError>({
    queryKey: classifyQueryKey(serviceId ?? 0),
    queryFn: () => classifyServicePolicies(serviceId as number),
    enabled: serviceId !== undefined,
    retry: false,
  });
  const rows = classified.data ?? [];

  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [preview, setPreview] = useState<MigrateResponse | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewedFor, setPreviewedFor] = useState<string | null>(null);
  const [migrateError, setMigrateError] = useState<string | null>(null);
  const [undoTarget, setUndoTarget] = useState<ClassifiedPolicy | null>(null);
  const [undoResult, setUndoResult] = useState<UndoResponse | null>(null);
  const [undoError, setUndoError] = useState<string | null>(null);

  // What is selected AND still selectable: a refetch can turn a selected row
  // into a migrated or blocked one, and it must then drop out of the request
  // (and so out of the snapshot the preview was taken for).
  const selectedNames = rows
    .filter((r) => selected.has(String(r.id)) && isSelectable(r))
    .map((r) => r.name)
    .sort();
  const selectionSnapshot = JSON.stringify(selectedNames);
  const previewStale = previewedFor !== selectionSnapshot;

  const invalidate = () => {
    if (serviceId !== undefined) {
      void queryClient.invalidateQueries({ queryKey: classifyQueryKey(serviceId) });
    }
    // A migration flips profiles to `generated` and switches originals off.
    void queryClient.invalidateQueries({ queryKey: ["control-plane", "profiles"] });
    void queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
  };

  const previewMutation = useMutation<MigrateResponse, ApiError, string[]>({
    mutationFn: (names) => migrateServicePolicies(serviceId as number, names, true),
  });
  const migrateMutation = useMutation<MigrateResponse, ApiError, string[]>({
    mutationFn: (names) => migrateServicePolicies(serviceId as number, names, false),
  });
  const undoMutation = useMutation<UndoResponse, ApiError, string>({
    mutationFn: (name) => undoServicePolicyMigration(serviceId as number, [name]),
  });

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function runPreview() {
    const snapshot = selectionSnapshot;
    setPreviewError(null);
    setPreview(null);
    setMigrateError(null);
    try {
      setPreview(await previewMutation.mutateAsync(selectedNames));
    } catch (e) {
      setPreviewError(writeErrorMessage(e, "The preview could not be run. Try again."));
    }
    setPreviewedFor(snapshot);
  }

  async function runMigrate() {
    const snapshot = selectionSnapshot;
    setMigrateError(null);
    try {
      const result = await migrateMutation.mutateAsync(selectedNames);
      if (result.committed) {
        const n = result.policies.length;
        toast.success(
          `Migrated ${n} ${n === 1 ? "policy" : "policies"}, proved by ${result.probe_evaluations} probe evaluations.`,
        );
        setSelected(new Set());
        setPreview(null);
        setPreviewedFor(null);
        invalidate();
      } else {
        // The gate ran again at commit time and did not prove it — the
        // profiles changed since the preview. Its answer replaces the
        // preview, so the operator reads the disagreements and Migrate stays
        // disabled (the response is not `equivalent`).
        setPreview(result);
        setPreviewedFor(snapshot);
      }
    } catch (e) {
      setMigrateError(writeErrorMessage(e, "The migration could not be run. Try again."));
    }
  }

  async function runUndo(target: ClassifiedPolicy) {
    setUndoError(null);
    setUndoResult(null);
    try {
      const result = await undoMutation.mutateAsync(target.name);
      toast.success(`Undid the migration of "${target.name}".`);
      setUndoResult(result);
      invalidate();
    } catch (e) {
      setUndoError(writeErrorMessage(e, "The undo could not be run. Try again."));
    }
  }

  const proved = !!preview && preview.equivalent && !previewError;
  const canMigrate =
    canWrite && selectedNames.length > 0 && !previewStale && proved && !migrateMutation.isPending;
  const migrateReason =
    selectedNames.length === 0
      ? "Select at least one migratable policy, then preview it."
      : previewStale && (preview || previewError)
        ? "The selection changed since that preview — preview it again before migrating."
        : previewStale
          ? "Preview the current selection before migrating."
          : previewError
            ? "The preview was refused, so this selection cannot be migrated."
            : preview && !preview.equivalent
              ? "The preview did not prove this migration, so it cannot be migrated."
              : null;

  const columns: Column<ClassifiedPolicy>[] = [
    {
      key: "name",
      label: "Policy",
      sortable: true,
      value: (r) => r.name,
      cell: (r) => (
        <div className="min-w-0">
          <span className="font-medium">{r.name}</span>
          {/* `enabled` is the column the engine reads. A migrated original is
              off too, but it is reported as migrated, never as merely off. */}
          {!r.enabled && !r.migrated && (
            <div className="text-xs text-muted-foreground">Switched off</div>
          )}
        </div>
      ),
    },
    {
      key: "owner",
      label: "Owner",
      sortable: true,
      value: (r) => r.managed_by,
      cell: (r) => <OwnerChip owner={r.managed_by} />,
    },
    {
      key: "verdict",
      label: "Verdict",
      sortable: true,
      value: (r) => verdictOf(r),
      cell: (r) => <VerdictBadge verdict={verdictOf(r)} />,
    },
    {
      key: "details",
      label: "Why",
      className: "min-w-[18rem] whitespace-normal",
      value: (r) => r.blockers.join(" "),
      cell: (r) => <RowDetails row={r} />,
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (r) =>
        r.migrated && canWrite ? (
          <Button
            size="sm"
            variant="outline"
            aria-label={`Undo migration of ${r.name}`}
            disabled={undoMutation.isPending}
            onClick={() => setUndoTarget(r)}
          >
            <Undo2 className="mr-1 h-4 w-4" />
            Undo
          </Button>
        ) : null,
    },
  ];

  const counts = {
    migratable: rows.filter(isSelectable).length,
    migrated: rows.filter((r) => !!r.migrated).length,
    locked: rows.filter((r) => verdictOf(r) === "Locked").length,
    blocked: rows.filter((r) => verdictOf(r) === "Blocked").length,
  };

  const profileMissing = profiles.isSuccess && !profile;

  return (
    <ListPageShell
      title="Migrate policies"
      description={`Profile ${service}: who owns each policy, whether it can move into the permission model, and a proof before it does.`}
      back={
        <Link
          to={`/governance/policies/${encodeURIComponent(service)}`}
          className="mb-1 inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" />
          {service || "Profile"}
        </Link>
      }
    >
      <div className="space-y-4">
        {!canWrite && (
          <Alert>
            <AlertDescription>
              Previewing, migrating and undoing needs the policy:update permission. This account
              can read what each policy is and why.
            </AlertDescription>
          </Alert>
        )}

        {profileMissing ? (
          <Alert variant="destructive">
            <AlertDescription>
              There is no policy profile named “{service}”. Go back to Policies and pick one.
            </AlertDescription>
          </Alert>
        ) : (
          <ListStates
            isLoading={profiles.isLoading || classified.isLoading}
            isError={profiles.isError || classified.isError}
            error={{ message: readErrorMessage(profiles.error ?? classified.error) }}
            errorText="The policies of this profile could not be loaded. Reload the page to try again."
            hasData={rows.length > 0}
            skeletonCols={5}
            empty={
              <EmptyState
                icon={FileText}
                variant="dashed"
                title="No policies in this profile"
                description="There is nothing to migrate: this profile holds no policies."
              />
            }
          >
            <div className="space-y-3">
              <SummaryStrip
                items={[
                  { label: "policies", singular: "policy", value: rows.length },
                  { label: "migratable", value: counts.migratable },
                  { label: "migrated", value: counts.migrated },
                  { label: "locked", value: counts.locked },
                  { label: "blocked", value: counts.blocked },
                ]}
              />
              <FilterableTable
                rows={rows}
                columns={columns}
                getId={(r) => String(r.id)}
                searchPlaceholder="Search policies…"
                noMatchText="No policy matches that search. Clear the search to see them all."
                bordered
                selection={
                  canWrite
                    ? {
                        selected,
                        onToggle: toggle,
                        canSelect: isSelectable,
                        rowLabel: (r) => `Select ${r.name}`,
                      }
                    : undefined
                }
              />
            </div>
          </ListStates>
        )}

        {undoError && (
          <Alert variant="destructive">
            <AlertDescription className="whitespace-pre-line">{undoError}</AlertDescription>
          </Alert>
        )}
        {undoResult && <UndoSummary result={undoResult} />}

        {canWrite && !profileMissing && rows.length > 0 && (
          <section
            aria-labelledby="migration-preview-heading"
            className="space-y-3 rounded-md border border-border bg-muted/30 p-4"
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h2 id="migration-preview-heading" className="text-base font-semibold">
                  Preview
                </h2>
                <p className="text-sm text-muted-foreground">
                  {selectedNames.length === 0
                    ? "No policy selected."
                    : `Selected: ${selectedNames.join(", ")}`}
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={selectedNames.length === 0 || previewMutation.isPending}
                  onClick={() => void runPreview()}
                >
                  {previewMutation.isPending && <Loader2 className="mr-1 h-4 w-4 animate-spin" />}
                  Preview migration
                </Button>
                <Button
                  type="button"
                  size="sm"
                  disabled={!canMigrate}
                  onClick={() => void runMigrate()}
                >
                  {migrateMutation.isPending && <Loader2 className="mr-1 h-4 w-4 animate-spin" />}
                  Migrate
                </Button>
              </div>
            </div>

            {migrateReason && <p className="text-xs text-muted-foreground">{migrateReason}</p>}

            {migrateError && (
              <Alert variant="destructive">
                <AlertDescription>{migrateError}</AlertDescription>
              </Alert>
            )}
            {previewError && !previewStale && (
              <Alert variant="destructive">
                <AlertDescription>{previewError}</AlertDescription>
              </Alert>
            )}
            {preview && !previewStale && (
              <PreviewResult preview={preview} currentServiceId={serviceId} />
            )}
          </section>
        )}
      </div>

      <AlertDialog
        open={undoTarget !== null}
        onOpenChange={(open) => {
          if (!open) setUndoTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Undo the migration of “{undoTarget?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              This deletes the grants and rules the migration created and switches the original
              policy back on. It is refused if any of them has been edited since.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (undoTarget) void runUndo(undoTarget);
                setUndoTarget(null);
              }}
            >
              Undo migration
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </ListPageShell>
  );
}

// ── row vocabulary ────────────────────────────────────────────────────────

type Verdict = "Migrated" | "Locked" | "Migratable" | "Blocked";

/** Only an operator's own, migratable, not-yet-migrated policy can be picked. */
function isSelectable(r: ClassifiedPolicy): boolean {
  return r.managed_by === "operator" && r.migratable && !r.migrated;
}

/**
 * The verdict LABEL. It names which of four states the server's fields put a
 * row in; WHY is never decided here — that is the row's blocker sentences.
 */
function verdictOf(r: ClassifiedPolicy): Verdict {
  if (r.migrated) return "Migrated";
  if (r.managed_by !== "operator") return "Locked";
  return r.migratable ? "Migratable" : "Blocked";
}

const OWNER_ICON: Record<string, typeof User> = {
  operator: User,
  infra: Server,
  compiler: Cog,
};

/** The owner, as stored (`managed_by`) — word and icon, never colour alone. */
function OwnerChip({ owner }: { owner: string }) {
  const Icon = OWNER_ICON[owner] ?? User;
  return (
    <Badge variant="outline" className="font-normal">
      <Icon aria-hidden />
      {owner}
    </Badge>
  );
}

function VerdictBadge({ verdict }: { verdict: Verdict }) {
  switch (verdict) {
    case "Migrated":
      return (
        <Badge variant="secondary">
          <CheckCircle2 aria-hidden />
          Migrated
        </Badge>
      );
    case "Locked":
      return (
        <Badge variant="outline">
          <Lock aria-hidden />
          Locked
        </Badge>
      );
    case "Migratable":
      return <Badge variant="default">Migratable</Badge>;
    case "Blocked":
      return (
        <Badge variant="outline" className="text-muted-foreground">
          Blocked
        </Badge>
      );
  }
}

/** The compiler's name prefixes, with what a policy under each one is. */
const COMPILED_KINDS: readonly (readonly [string, string, string])[] = [
  ["perm-grant-", "compiled grant policy", "compiled grant policies"],
  ["perm-rule-", "compiled rule policy", "compiled rule policies"],
  ["perm-clearance-", "compiled clearance policy", "compiled clearance policies"],
];

/**
 * What a profile's download now serves. A compiled policy's name embeds its
 * grant or rule id, and an unwritten preview's ids belong to a write that was
 * rolled back — they will never exist. So until the migration is written, the
 * compiled policies are counted by kind, not named.
 */
function describeServed(names: string[], written: boolean): string {
  if (written) return names.join(", ") || "—";
  const counts = COMPILED_KINDS.map(() => 0);
  const others: string[] = [];
  for (const name of names) {
    const kind = COMPILED_KINDS.findIndex(([prefix]) => name.startsWith(prefix));
    if (kind >= 0) counts[kind] += 1;
    else others.push(name);
  }
  const compiled = COMPILED_KINDS.flatMap(([, one, many], i) =>
    counts[i] > 0 ? [plural(counts[i], one, many)] : [],
  );
  const parts = [...compiled, ...others];
  if (parts.length === 0) return "—";
  const text = parts.join(", ");
  return compiled.length > 0 ? `${text} (named when written)` : text;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function RowDetails({ row }: { row: ClassifiedPolicy }) {
  return (
    <div className="space-y-1 text-sm">
      {row.blockers.length > 0 && (
        <ul className="space-y-1">
          {row.blockers.map((b, i) => (
            // Index in the key: two blockers can render the same sentence.
            <li key={`${i}-${b}`}>{b}</li>
          ))}
        </ul>
      )}
      {row.migrated && (
        <p className="text-xs text-muted-foreground">
          Proved by <span className="tabular-nums">{row.migrated.probes_proved}</span> probe
          evaluations.
        </p>
      )}
      {!row.migrated && row.migratable && row.proposed && (
        <p className="text-muted-foreground">
          Becomes {plural(row.proposed.grants, "grant", "grants")} and{" "}
          {plural(row.proposed.rules, "rule", "rules")}.
        </p>
      )}
    </div>
  );
}

// ── preview ───────────────────────────────────────────────────────────────

const PLUGIN_VERSION_LABEL: Record<string, string> = {
  ranger_2_5_0: "Ranger 2.5.0",
  ranger_2_8_0: "Ranger 2.8.0",
};
const CHECK_LABEL: Record<string, string> = {
  access: "Access",
  data_mask: "Mask",
  row_filter: "Row filter",
};
const TAGGING_LABEL: Record<string, string> = {
  actual: "Actual",
  hypothetical: "Hypothetical",
};
const versionLabel = (v: string) => PLUGIN_VERSION_LABEL[v] ?? v;

function PreviewResult({
  preview,
  currentServiceId,
}: {
  preview: MigrateResponse;
  currentServiceId: number | undefined;
}) {
  const notCommitted = !preview.dry_run && !preview.committed;
  return (
    <div className="space-y-4 text-sm">
      {preview.refusal ? (
        <Alert variant="destructive">
          <AlertDescription>
            <p className="font-medium">
              The gate could not prove this migration. Nothing was written.
            </p>
            <p>{preview.refusal}</p>
          </AlertDescription>
        </Alert>
      ) : preview.disagreements.length > 0 ? (
        <Alert variant="destructive">
          <AlertDescription>
            {notCommitted
              ? "Migrating found that the profiles changed since the preview: "
              : ""}
            The migration would change who can do what on{" "}
            {plural(preview.disagreements.length, "probe", "probes")}, listed below. Nothing was
            written. Fix the policies or the model, then preview again.
          </AlertDescription>
        </Alert>
      ) : preview.equivalent ? (
        <p>
          Proved equivalent:{" "}
          <span className="tabular-nums">{preview.probe_evaluations}</span> probe evaluations
          agreed on every affected profile. Nothing has been written yet.
        </p>
      ) : null}

      {preview.notices.length > 0 && (
        <Alert>
          <AlertDescription>
            <p className="font-medium">Before you migrate</p>
            <ul className="mt-1 space-y-1">
              {preview.notices.map((n, i) => (
                <li key={`${i}-${n}`}>{n}</li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}

      {preview.profiles.length > 0 && (
        <AffectedProfiles
          profiles={preview.profiles}
          currentServiceId={currentServiceId}
          written={preview.committed}
        />
      )}

      {preview.disagreements.length > 0 && <Disagreements rows={preview.disagreements} />}

      {preview.policies.length > 0 && (
        <div className="space-y-1">
          <h3 className="font-medium">Policies and what they become</h3>
          <ul className="space-y-0.5 text-muted-foreground">
            {preview.policies.map((p) => (
              <li key={p.policy_id}>
                {p.policy_name}: {plural(p.grant_ids.length, "grant", "grants")},{" "}
                {plural(p.rule_ids.length, "rule", "rules")}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/**
 * EVERY profile the migration changes — not just the one on screen. A grant
 * compiles into every profile that serves its catalog, and a tag rule into
 * every profile sharing the tag service, so the operator is changing more
 * than this page's profile; the ones that are not it are marked.
 */
function AffectedProfiles({
  profiles,
  currentServiceId,
  written,
}: {
  profiles: GatedProfile[];
  currentServiceId: number | undefined;
  written: boolean;
}) {
  const n = profiles.length;
  return (
    <section aria-label="Affected profiles" className="space-y-2">
      <h3 className="font-medium">
        This migration changes {n} {n === 1 ? "profile" : "profiles"}
      </h3>
      <div className="rounded-md border bg-background">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Profile</TableHead>
              <TableHead>Plugin versions</TableHead>
              <TableHead className="text-right">Probes</TableHead>
              <TableHead className="text-right">Disagreements</TableHead>
              <TableHead>Now serves</TableHead>
              <TableHead>No longer serves</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {profiles.map((p) => (
              <TableRow key={p.service_id}>
                <TableCell>
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="font-medium">{p.service_name}</span>
                    {p.service_id !== currentServiceId && (
                      <Badge variant="outline">Also changed</Badge>
                    )}
                  </div>
                </TableCell>
                <TableCell>
                  <div className="flex flex-wrap gap-1">
                    {p.plugin_versions.map((v) => (
                      <Badge key={v} variant="secondary" className="font-normal">
                        {versionLabel(v)}
                      </Badge>
                    ))}
                  </div>
                </TableCell>
                <TableCell className="text-right tabular-nums">{p.probes}</TableCell>
                <TableCell className="text-right tabular-nums">{p.disagreements}</TableCell>
                <TableCell className="whitespace-normal font-mono text-xs">
                  {describeServed(p.added_policies, written)}
                </TableCell>
                <TableCell className="whitespace-normal font-mono text-xs">
                  {p.removed_policies.join(", ") || "—"}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </section>
  );
}

/** Each probe on which a profile answers differently after the write: the
 *  server's sentence, then the probe's fields, so it can be reproduced. */
function Disagreements({ rows }: { rows: WireDisagreement[] }) {
  return (
    <div className="space-y-2">
      <h3 className="font-medium">Disagreements</h3>
      <ul className="list-disc space-y-1 pl-5">
        {rows.map((d, i) => (
          <li key={`${i}-${d.sentence}`}>{d.sentence}</li>
        ))}
      </ul>
      <div className="rounded-md border bg-background">
        <Table aria-label="Disagreements">
          <TableHeader>
            <TableRow>
              <TableHead>Profile</TableHead>
              <TableHead>Plugin version</TableHead>
              <TableHead>Check</TableHead>
              <TableHead>User</TableHead>
              <TableHead>Access type</TableHead>
              <TableHead>Resource</TableHead>
              <TableHead>Tagging</TableHead>
              <TableHead>Before</TableHead>
              <TableHead>After</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((d, i) => (
              <TableRow key={`${i}-${d.sentence}`}>
                <TableCell>{d.service_name}</TableCell>
                <TableCell>{versionLabel(d.plugin_version)}</TableCell>
                <TableCell>{CHECK_LABEL[d.check] ?? d.check}</TableCell>
                <TableCell>
                  <div className="font-medium">{d.user}</div>
                  {(d.groups.length > 0 || d.roles.length > 0 || d.tags.length > 0) && (
                    <div className="text-xs text-muted-foreground">
                      {[
                        d.groups.length > 0 ? `groups: ${d.groups.join(", ")}` : null,
                        d.roles.length > 0 ? `roles: ${d.roles.join(", ")}` : null,
                        d.tags.length > 0 ? `tags: ${d.tags.join(", ")}` : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </div>
                  )}
                </TableCell>
                <TableCell>{d.access_type}</TableCell>
                <TableCell className="font-mono text-xs">{d.resource}</TableCell>
                <TableCell>{TAGGING_LABEL[d.tagging] ?? d.tagging}</TableCell>
                <TableCell>{d.before}</TableCell>
                <TableCell>{d.after}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}

function UndoSummary({ result }: { result: UndoResponse }) {
  return (
    <Alert>
      <AlertDescription>
        <ul className="space-y-1">
          {result.policies.map((p) => (
            <li key={p.policy_id}>
              Undid “{p.policy_name}”: deleted {plural(p.grants_deleted, "grant", "grants")} and{" "}
              {plural(p.rules_deleted, "rule", "rules")}, and switched the original back on.
            </li>
          ))}
          {result.notices.map((n, i) => (
            <li key={`${i}-${n}`}>{n}</li>
          ))}
        </ul>
      </AlertDescription>
    </Alert>
  );
}

// ── errors ────────────────────────────────────────────────────────────────

/**
 * What a refused preview, migrate or undo says. A 409's text (already
 * migrated, a rule name taken, undo found changes — each naming exactly what)
 * and a 400's are the server's; a 403 names the permission; the bare
 * "not found" and "internal error" bodies say nothing an operator can act on,
 * so those two say what to do instead.
 */
function writeErrorMessage(e: unknown, fallback: string): string {
  const status = (e as ApiError | undefined)?.status;
  if (status === 404) {
    return "The profile, or one of these policies, no longer exists. Reload the page to see the current list.";
  }
  if (status === 500) {
    return "The server failed while doing this, and nothing was written. Try again; if it keeps failing, check the server log.";
  }
  return refusalMessage(e, "policy:update", fallback);
}

function readErrorMessage(e: ApiError | null | undefined): string {
  if (!e) return "";
  if (e.status === 403) return "This account cannot read policies: it needs the policy:read permission.";
  if (e.status === 404) return "This profile no longer exists. Go back to Policies and pick another.";
  return e.message;
}
