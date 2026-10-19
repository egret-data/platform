import { useState, type ComponentType } from "react";
import { AlertTriangle, BellOff, CheckCircle2, CircleHelp, CircleMinus, Clock, Download, PauseCircle } from "lucide-react";
import { useControlPlaneGovernanceEnginePlugin } from "@/api/generated/control-plane-governance/control-plane-governance";
import type { KindReport, KindStateDto, LedgerRowDto } from "@/api/generated/model";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { EmptyState } from "@/components/explore/EmptyState";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";
import { Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { NEEDS_ENGINE_READ, type TabContext } from "@/routes/governance/engines/PageFrame";

/** The word for a kind's state; the index's plugin column uses it too. */
export const KIND_STATE_WORD: Record<KindStateDto, string> = {
  up_to_date: "Up to date",
  pending: "Pending",
  behind: "Behind",
  quiet: "Quiet",
  silent: "Silent",
  not_reported: "Not reported",
  not_expected: "Not expected",
};

const NOT_REPORTED_WORD: Record<string, string> = {
  no_download: "Not reported: no download in 24 hours",
  version_unreported: "Not reported: no version reported yet",
};

const STATE_ICON: Record<KindStateDto, ComponentType<{ className?: string; "aria-hidden"?: boolean }>> = {
  up_to_date: CheckCircle2,
  pending: Clock,
  behind: AlertTriangle,
  quiet: PauseCircle,
  silent: BellOff,
  not_reported: CircleHelp,
  not_expected: CircleMinus,
};

const STATE_TONE: Partial<Record<KindStateDto, string>> = {
  up_to_date: "text-success",
  behind: "text-destructive",
  silent: "text-destructive",
};

const KIND_LABEL: Record<string, string> = { policies: "Policies", tags: "Tags" };
const KIND_ORDER = ["policies", "tags"];

export const HELD_RULE =
  "A version counts as held once a plugin reports it; confirmation takes two polls, about a minute at the default 30 s poll.";

function stateWord(k: KindReport): string {
  if (k.state === "not_reported" && k.notReported) return NOT_REPORTED_WORD[k.notReported] ?? KIND_STATE_WORD.not_reported;
  return KIND_STATE_WORD[k.state];
}

/** A moment as `<time>`: relative text, the absolute one on hover and in `dateTime`. */
function When({ iso }: { iso: string }) {
  return (
    <time dateTime={iso} title={fullTimestamp(iso)}>
      {timeAgo(iso)}
    </time>
  );
}

function Version({ v }: { v: number | null | undefined }) {
  return v === null || v === undefined ? <span className="text-muted-foreground">—</span> : <span className="tabular-nums">{v}</span>;
}

function KindsTable({ kinds }: { kinds: KindReport[] }) {
  const sorted = [...kinds].sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind));
  return (
    <Table aria-label="Download state per kind" className={STACKED_TABLE}>
      <TableHeader>
        <TableRow>
          <TableHead>Kind</TableHead>
          <TableHead>State</TableHead>
          <TableHead>Last download</TableHead>
          <TableHead>Current version</TableHead>
          <TableHead>Newest reported</TableHead>
          <TableHead>Served</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sorted.map((k) => {
          const Icon = STATE_ICON[k.state];
          return (
            <TableRow key={k.kind} aria-label={KIND_LABEL[k.kind] ?? k.kind}>
              <TableCell className="font-medium">{KIND_LABEL[k.kind] ?? k.kind}</TableCell>
              <TableCell data-label="State" className="whitespace-normal">
                <span className="inline-flex items-center gap-1.5 font-medium">
                  <Icon className={`h-4 w-4 shrink-0 ${STATE_TONE[k.state] ?? "text-muted-foreground"}`} aria-hidden />
                  <span>{stateWord(k)}</span>
                </span>
                <p className="mt-0.5 text-sm text-muted-foreground">{k.sentence}</p>
              </TableCell>
              <TableCell data-label="Last download">
                {k.lastDownloadAt ? <When iso={k.lastDownloadAt} /> : <span className="text-muted-foreground">None in 24 hours</span>}
              </TableCell>
              <TableCell data-label="Current version">
                <Version v={k.currentVersion} />
              </TableCell>
              <TableCell data-label="Newest reported">
                <Version v={k.newestReportedVersion} />
              </TableCell>
              <TableCell data-label="Served">
                <Version v={k.servedVersion} />
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function RawRows({ rows }: { rows: LedgerRowDto[] }) {
  if (rows.length === 0) {
    return <EmptyState title="No plugin has downloaded from this profile in the last 24 hours." />;
  }
  return (
    <Table aria-label="Downloads reported by plugins" className={STACKED_TABLE}>
      <TableHeader>
        <TableRow>
          <TableHead>Plugin</TableHead>
          <TableHead>Kind</TableHead>
          <TableHead>Last known version</TableHead>
          <TableHead>Served</TableHead>
          <TableHead>When</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={`${r.pluginId} ${r.kind}`}>
            <TableCell className="break-all font-mono text-xs">{r.pluginId}</TableCell>
            <TableCell data-label="Kind">{KIND_LABEL[r.kind] ?? r.kind}</TableCell>
            <TableCell data-label="Last known version">
              {/* -1 is the plugin's "none yet" (Overview live check, finding 2). */}
              <Version v={r.lastKnownVersion === -1 ? null : r.lastKnownVersion} />
            </TableCell>
            <TableCell data-label="Served">
              <Version v={r.servedVersion} />
            </TableCell>
            <TableCell data-label="When">
              <When iso={r.at} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/**
 * Plugin (spec §6.5, EP8, EP17–EP19): what the engine's Ranger plugins report
 * downloading, judged by the same function as the Overview's findings
 * (`GET /governance/engines/{id}/plugin`). It records what plugins report,
 * never what an engine enforces. Without `engine:read` the state is replaced
 * by one line (EP19); the Trino config download, which needs `policy:read`
 * only, stays (EP18).
 */
export function PluginTab({ ctx }: { ctx: TabContext }) {
  const can = usePolicyPermissions();
  const engineRead = can("engine:read");
  const engineId = "engineId" in ctx.subject ? ctx.subject.engineId : "";
  const q = useControlPlaneGovernanceEnginePlugin(engineId, { query: { enabled: engineRead && !!engineId } });
  const [configOpen, setConfigOpen] = useState(false);

  const data = q.data?.status === 200 ? q.data.data : null;
  const profileName = data?.profile?.name ?? ctx.profile?.name ?? null;
  const isTrino = (data?.engine.type ?? ctx.engine?.type ?? ctx.profile?.type) === "trino";

  const trinoConfig =
    isTrino && profileName ? (
      <>
        <Button size="sm" variant="outline" onClick={() => setConfigOpen(true)}>
          <Download className="mr-1 h-4 w-4" aria-hidden />
          Download Trino config
        </Button>
        <TrinoConfigDialog open={configOpen} service={profileName} onOpenChange={setConfigOpen} />
      </>
    ) : null;

  let body;
  if (!engineRead) {
    body = <p className="text-sm text-muted-foreground">{NEEDS_ENGINE_READ}</p>;
  } else if (q.isError) {
    body = <TabError what="plugin state" onRetry={() => void q.refetch()} />;
  } else if (!data) {
    body = (
      <div data-testid="plugin-loading">
        <ListSkeleton rows={3} cols={4} />
      </div>
    );
  } else {
    body = (
      <div className="space-y-6">
        <p className="flex flex-wrap gap-x-6 gap-y-1 text-sm">
          <span>
            <span className="text-muted-foreground">Ranger plugin </span>
            <span>{data.plugin.version ?? "version unknown"}</span>
          </span>
          <span>{data.plugin.modelled ? "Modelled by egret" : (data.plugin.note ?? "Not modelled by egret")}</span>
        </p>
        {data.judged.state === "not_judged" ? (
          <p className="text-sm">{data.judged.sentence}</p>
        ) : (
          <Section title="Download state">
            <KindsTable kinds={data.kinds} />
            <p className="text-sm text-muted-foreground">{HELD_RULE}</p>
          </Section>
        )}
        {data.profile ? (
          <Section title="Downloads reported by plugins">
            {data.sharedWith.length > 0 ? (
              <p className="text-sm text-muted-foreground">{`These rows may come from any engine bound to ${data.profile.name}.`}</p>
            ) : null}
            <RawRows rows={data.rows} />
          </Section>
        ) : null}
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {trinoConfig ? <div className="flex justify-end">{trinoConfig}</div> : null}
      {body}
    </div>
  );
}
