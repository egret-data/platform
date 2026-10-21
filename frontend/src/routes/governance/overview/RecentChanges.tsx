import { useState } from "react";
import { Link } from "react-router-dom";
import { History } from "lucide-react";
import {
  controlPlaneGovernanceChanges,
  useControlPlaneGovernanceChanges,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { ChangeEntry, Party } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";
import { useIsNarrowViewport } from "@/lib/useIsNarrowViewport";
import { TERM } from "@/routes/governance/glossary";
import { LINK } from "@/routes/governance/overview/AttentionRow";
import { changeSentence } from "@/routes/governance/overview/changeSentence";
import { refUrl } from "@/routes/governance/overview/refUrl";

const PAGE = 25;

const name = (p: Party | null | undefined) => (p ? (p.label ?? "A deleted user") : "egret");

/** "Ana", "Admin as Ana", or "A deleted user". */
export function who(e: ChangeEntry): string {
  return e.impersonator ? `${name(e.impersonator)} as ${name(e.actor)}` : name(e.actor);
}

function When({ at }: { at: string }) {
  return (
    <time dateTime={at} title={fullTimestamp(at)} className="tabular-nums">
      {timeAgo(at)}
    </time>
  );
}

function Target({ e }: { e: ChangeEntry }) {
  if (!e.target) return null;
  if (!e.targetExists) return <span className="text-muted-foreground">(deleted)</span>;
  const url = refUrl(e.target);
  return url ? (
    <Link to={url} className={LINK}>
      View
    </Link>
  ) : null;
}

/**
 * Recent changes (spec §2): governance events, newest first, 25 a page.
 * "Show older" follows the server's keyset cursor and appends; an entry seen
 * twice (a page boundary moved) is kept once.
 */
export function RecentChanges() {
  const narrow = useIsNarrowViewport();
  const first = useControlPlaneGovernanceChanges({ limit: PAGE }, { query: { retry: false } });
  // Pages appended by "Show older", tagged with the first page they follow.
  // When the first page is fetched again (a refetch, a retry), they belong to
  // a list that no longer exists and are dropped (review M8).
  const [appended, setAppended] = useState<{ for: number; entries: ChangeEntry[]; cursor: string | null }>({
    for: 0,
    entries: [],
    cursor: null,
  });
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [olderError, setOlderError] = useState<string | null>(null);

  if (first.isError) {
    return (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center justify-between gap-2">
          <span>{(first.error as unknown as Error | undefined)?.message || "Could not load recent changes."}</span>
          <Button size="sm" variant="outline" onClick={() => void first.refetch()}>
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  }
  const page = first.data?.status === 200 ? first.data.data : undefined;
  if (first.isLoading || !page) return <ListSkeleton cols={3} rows={4} />;

  const current = appended.for === first.dataUpdatedAt;
  const older = current ? appended.entries : [];
  const seen = new Set<number>();
  const entries = [...page.entries, ...older].filter((e) => (seen.has(e.id) ? false : (seen.add(e.id), true)));
  const next = current ? appended.cursor : page.nextCursor;
  const forPage = first.dataUpdatedAt;

  async function showOlder() {
    if (!next) return;
    setLoadingOlder(true);
    setOlderError(null);
    try {
      // FE-DATA-2 exception: a keyset cursor across pages, appended in place.
      const r = await controlPlaneGovernanceChanges({ limit: PAGE, cursor: next });
      if (r.status === 200) {
        setAppended((a) => ({
          for: forPage,
          entries: [...(a.for === forPage ? a.entries : []), ...r.data.entries],
          cursor: r.data.nextCursor ?? null,
        }));
      }
    } catch (e) {
      setOlderError((e as Error).message || "Could not load older changes.");
    } finally {
      setLoadingOlder(false);
    }
  }

  if (entries.length === 0) {
    return (
      <EmptyState
        icon={History}
        title="No governance change has been recorded yet."
        description="Recording began when this version was installed; earlier changes are not listed."
      />
    );
  }

  // Live check D13: a reader sees only the families they may read, and pruning
  // removes the oldest, so the oldest entry shown is not when recording began.
  // Once every page is loaded, say what is known: the oldest change shown.
  const oldest = next ? null : entries[entries.length - 1];
  return (
    <div className="flex flex-col gap-3">
      {narrow ? (
        <ul className="flex flex-col gap-2" data-testid="recent-changes-cards">
          {entries.map((e) => (
            <li key={e.id} className="flex flex-col gap-1 rounded-md border border-border p-3 text-sm">
              <div className="flex flex-wrap items-center justify-between gap-2 text-muted-foreground">
                <span>{who(e)}</span>
                <When at={e.at} />
              </div>
              <p className="break-words">{changeSentence(e, who(e))}</p>
              <Target e={e} />
            </li>
          ))}
        </ul>
      ) : (
        <Table data-testid="recent-changes-table">
          <TableHeader>
            <TableRow>
              <TableHead>When</TableHead>
              <TableHead>Who</TableHead>
              <TableHead>What</TableHead>
              <TableHead>
                <span className="sr-only">Link</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {entries.map((e) => (
              <TableRow key={e.id}>
                <TableCell className="whitespace-nowrap">
                  <When at={e.at} />
                </TableCell>
                <TableCell>{who(e)}</TableCell>
                <TableCell className="whitespace-normal">{changeSentence(e, who(e))}</TableCell>
                <TableCell>
                  <Target e={e} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {olderError ? (
        <Alert variant="destructive">
          <AlertDescription>{olderError}</AlertDescription>
        </Alert>
      ) : null}
      {next ? (
        <div>
          <Button variant="outline" size="sm" onClick={() => void showOlder()} disabled={loadingOlder}>
            Show older
          </Button>
        </div>
      ) : null}
      <div className="flex flex-col gap-1 text-xs text-muted-foreground">
        <p>
          {oldest ? `The oldest change you can see is from ${fullTimestamp(oldest.at).slice(0, 10)}. ` : null}
          {page.retentionDays === null || page.retentionDays === undefined
            ? "Changes are never pruned."
            : `Changes older than ${page.retentionDays} days are pruned.`}
        </p>
        <p>
          {`Role assignments and ${TERM.classifications.toLowerCase()} attached to data are recorded after they commit and can be missing if the audit writer was overloaded.`}
        </p>
      </div>
    </div>
  );
}
