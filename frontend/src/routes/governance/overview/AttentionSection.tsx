import { Link } from "react-router-dom";
import type { AttentionResponse, Finding, NotChecked, Severity } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { AttentionRow, LINK, SEVERITY } from "@/routes/governance/overview/AttentionRow";

/** What each check looks at, for the "Not checked for you" lines. */
const CHECKS: Record<string, string> = {
  unresolved_principals_fail_open: "grant principals that match nobody",
  plugin_silent: "engine liveness and plugin drift",
  plugin_behind: "engine liveness and plugin drift",
  plugin_quiet: "engine liveness and plugin drift",
  compile_failed: "profile compiles",
  engine_failed: "engine status",
  engine_auth_failing: "engine Ranger credentials",
  rule_overlaps: "rule overlaps",
  lineage_failed: "view lineage",
  catalog_apply_failed: "catalog attachments",
  policy_warnings: "policy warnings",
  plugin_unmodelled: "engine plugin versions",
  tags_without_clearance: "classification clearances",
  catalogs_without_engine: "catalogs without an engine",
  username_reuse: "engine usernames reused after a delete",
};

/** One line per check the reader may not run; checks sharing a sentence share a line. */
export function notCheckedLines(notChecked: NotChecked[]): string[] {
  const lines: string[] = [];
  for (const n of notChecked) {
    const perms = n.requires.map((p) => `\`${p}\``).join(n.any ? " or " : " and ");
    const line = `Not checked for you: ${CHECKS[n.kind] ?? n.kind} (needs ${perms}).`;
    if (!lines.includes(line)) lines.push(line);
  }
  return lines;
}

const ORDER: Severity[] = ["critical", "warning", "info"];

/**
 * Needs attention (spec §2): findings grouped Critical, Warning, Info, a group
 * with no finding not rendered; then the checks this reader cannot run, and
 * the fixed list of what the Overview never checks.
 *
 * The empty state is derived from the content (FE-STATE-4): "Nothing needs
 * attention" only when there is no finding at all, a `source_unavailable` one
 * included, and "among the checks you can run" when some were not run, so a
 * response with only `notChecked` entries never claims a clean bill.
 */
export function AttentionSection({
  isLoading,
  isError,
  error,
  data,
  onRetry,
  canReadTags,
}: {
  isLoading: boolean;
  isError: boolean;
  error: unknown;
  data: AttentionResponse | undefined;
  onRetry: () => void;
  canReadTags: boolean;
}) {
  if (isError) {
    return (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center justify-between gap-2">
          <span>{(error as Error | undefined)?.message || "Could not load what needs attention."}</span>
          <Button size="sm" variant="outline" onClick={onRetry}>
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  }
  if (isLoading || !data) return <ListSkeleton cols={1} rows={3} />;

  const groups = ORDER.map((sev) => ({
    sev,
    findings: data.findings.filter((f: Finding) => f.severity === sev),
  })).filter((g) => g.findings.length > 0);
  const lines = notCheckedLines(data.notChecked);

  return (
    <div className="flex flex-col gap-4">
      {groups.length === 0 ? (
        <EmptyState
          title={lines.length > 0 ? "Nothing needs attention among the checks you can run." : "Nothing needs attention."}
        />
      ) : (
        groups.map(({ sev, findings }) => (
          <section key={sev} aria-labelledby={`attention-${sev}`} className="flex flex-col gap-2">
            <h3 id={`attention-${sev}`} className="text-sm font-semibold">
              {SEVERITY[sev].word}
            </h3>
            <ul className="flex flex-col gap-2">
              {findings.map((f) => (
                <AttentionRow key={`${f.kind}-${f.source ?? ""}`} finding={f} />
              ))}
            </ul>
          </section>
        ))
      )}
      <div className="flex flex-col gap-1 text-sm text-muted-foreground">
        {lines.map((l) => (
          <p key={l}>{l}</p>
        ))}
        <p>
          Not checked here: views that may expose classified data through DEFINER (
          {canReadTags ? (
            <Link to="/governance/classifications" className={LINK}>
              see Classifications
            </Link>
          ) : (
            "see Classifications"
          )}
          ), and classification coverage.
        </p>
      </div>
    </div>
  );
}
