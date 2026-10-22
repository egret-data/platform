import { useEffect, useMemo, useState } from "react";

import { useControlPlaneAuditListAudit } from "@/api/generated/control-plane-audit/control-plane-audit";
import type { AuditEntryDto } from "@/api/generated/model/auditEntryDto";
import type { ControlPlaneAuditListAuditParams } from "@/api/generated/model/controlPlaneAuditListAuditParams";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const COLUMNS = ["Time", "Actor", "Action", "Resource", "Decision", "Detail"];

const PAGE_SIZE = 100;

/** datetime-local value → ISO 8601, or undefined when empty/invalid. */
function toIso(local: string): string | undefined {
  if (!local) return undefined;
  const d = new Date(local);
  return Number.isNaN(d.getTime()) ? undefined : d.toISOString();
}

/**
 * Control Plane → System → Audit log.
 *
 * Every authorization decision and grant change, newest first. Reads the
 * platform-admin-only `GET /control-plane/audit` keyset-paginated feed, with
 * kind / decision / action filters. Changing any filter resets pagination.
 *
 * It opens on "Changes and denials" (KI-017): every page view writes an
 * allowed authorization decision, and those bury the grant changes.
 */
export default function AuditLog() {
  // Filters. "all" means "omit the param"; kind "changes" means
  // `hideAllowed` (mutations, plus decisions that refused).
  const [kind, setKind] = useState("changes");
  const [decision, setDecision] = useState("all");
  const [action, setAction] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");

  // Keyset pagination: accumulate pages, remember the next cursor.
  const [entries, setEntries] = useState<AuditEntryDto[]>([]);
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const [nextCursor, setNextCursor] = useState<string | null>(null);

  const actionParam = action.trim() || undefined;
  const fromIso = toIso(from);
  const toIsoValue = toIso(to);

  const params: ControlPlaneAuditListAuditParams = useMemo(
    () => ({
      kind: kind === "all" || kind === "changes" ? undefined : kind,
      hideAllowed: kind === "changes" ? true : undefined,
      decision: decision === "all" ? undefined : decision,
      action: actionParam,
      from: fromIso,
      to: toIsoValue,
      cursor,
      limit: PAGE_SIZE,
    }),
    [kind, decision, actionParam, fromIso, toIsoValue, cursor],
  );

  const query = useControlPlaneAuditListAudit(params, {
    query: { retry: false },
  });
  // `apiFetch` throws on non-2xx, so a resolved query is always the success
  // envelope; narrow on the 200 status to peel the `AuditPage` out of the
  // generated success|error response union.
  const page = query.data?.status === 200 ? query.data.data : undefined;

  // Any filter change resets the accumulated feed + cursor. `cursor` is
  // deliberately NOT a dependency: paging forward must preserve prior rows.
  useEffect(() => {
    setEntries([]);
    setCursor(undefined);
    setNextCursor(null);
  }, [kind, decision, actionParam, fromIso, toIsoValue]);

  // Append each freshly-fetched page, deduped by id (guards StrictMode double
  // effects and any accidental key overlap).
  useEffect(() => {
    if (!page) return;
    setEntries((prev) => {
      const seen = new Set(prev.map((e) => e.id));
      const merged = prev.slice();
      for (const e of page.entries) {
        if (!seen.has(e.id)) merged.push(e);
      }
      return merged;
    });
    setNextCursor(page.nextCursor ?? null);
  }, [page]);

  const isInitialLoading = query.isLoading && entries.length === 0;
  const errorMessage =
    (query.error as { message?: string } | null)?.message ??
    "Failed to load the audit log.";

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Audit log</h1>
        <p className="text-sm text-muted-foreground">
          Every authorization decision and grant change, newest first.
          Platform-admin only.
        </p>
      </div>

      <div className="space-y-4">
          {/* Filter bar */}
          <div className="flex flex-wrap items-end gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="audit-kind">Kind</Label>
              <Select value={kind} onValueChange={setKind}>
                <SelectTrigger id="audit-kind" className="w-48" aria-label="Kind">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="changes">Changes and denials</SelectItem>
                  <SelectItem value="all">All</SelectItem>
                  <SelectItem value="decision">Decision</SelectItem>
                  <SelectItem value="mutation">Mutation</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="audit-decision">Decision</Label>
              <Select value={decision} onValueChange={setDecision}>
                <SelectTrigger
                  id="audit-decision"
                  className="w-36"
                  aria-label="Decision"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All</SelectItem>
                  <SelectItem value="allow">Allow</SelectItem>
                  <SelectItem value="deny">Deny</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="audit-action">Action</Label>
              <Input
                id="audit-action"
                value={action}
                onChange={(e) => setAction(e.target.value)}
                placeholder="e.g. secret:read"
                className="w-52"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="audit-from">From</Label>
              <Input
                id="audit-from"
                type="datetime-local"
                value={from}
                onChange={(e) => setFrom(e.target.value)}
                className="w-52"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="audit-to">To</Label>
              <Input
                id="audit-to"
                type="datetime-local"
                value={to}
                onChange={(e) => setTo(e.target.value)}
                className="w-52"
              />
            </div>
          </div>

          {query.isError && entries.length === 0 ? (
            <Alert variant="destructive">
              <AlertDescription>{errorMessage}</AlertDescription>
            </Alert>
          ) : null}

          {isInitialLoading ? (
            <div className="space-y-2">
              {Array.from({ length: 6 }).map((_, i) => (
                <Skeleton key={i} className="h-9 w-full" />
              ))}
            </div>
          ) : (
            <div className="overflow-x-auto rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    {COLUMNS.map((col) => (
                      <TableHead key={col}>{col}</TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {entries.map((e) => (
                    <AuditRow key={e.id} entry={e} />
                  ))}
                  {entries.length === 0 && !query.isError && (
                    <TableRow className="hover:bg-transparent">
                      <TableCell
                        colSpan={COLUMNS.length}
                        className="py-10 text-center text-muted-foreground"
                      >
                        No audit entries match the filters.
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </div>
          )}

          {entries.length > 0 && (
            <div className="flex items-center justify-between gap-2 text-sm text-muted-foreground">
              <span>
                Showing <span className="tabular-nums">{entries.length}</span> event
                {entries.length === 1 ? "" : "s"}
                {nextCursor !== null ? " so far" : ""}
              </span>
              {nextCursor !== null && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={query.isFetching}
                  onClick={() => setCursor(nextCursor)}
                >
                  {query.isFetching ? "Loading…" : "Load more"}
                </Button>
              )}
            </div>
          )}
      </div>
    </div>
  );
}

function AuditRow({ entry }: { entry: AuditEntryDto }) {
  const detail =
    entry.kind === "decision"
      ? entry.reasons.join(", ")
      : JSON.stringify(entry.detail);
  const detailTruncated =
    detail.length > 96 ? `${detail.slice(0, 96)}…` : detail;

  // The API names the actor; a short id remains when the user is gone.
  const actorEmail = entry.actorEmail ?? undefined;

  return (
    <TableRow>
      <TableCell className="whitespace-nowrap tabular-nums text-muted-foreground">
        <RelativeTime iso={entry.ts} />
      </TableCell>
      <TableCell>
        {entry.actorUserId ? (
          <span className="truncate" title={entry.actorUserId}>
            {actorEmail ?? (
              <span className="font-mono text-xs">{entry.actorUserId.slice(0, 8)}</span>
            )}
          </span>
        ) : (
          <span className="text-muted-foreground">system</span>
        )}
      </TableCell>
      <TableCell className="font-mono text-xs font-medium">{entry.action}</TableCell>
      <TableCell>
        {entry.resourceType}
        {entry.resourceId ? (
          <span className="text-muted-foreground"> ({entry.resourceId})</span>
        ) : null}
      </TableCell>
      <TableCell>
        {entry.decision === "allow" ? (
          <Badge
            className="border-transparent"
            style={{
              backgroundColor: "var(--success)",
              color: "var(--success-foreground)",
            }}
          >
            allow
          </Badge>
        ) : entry.decision === "deny" ? (
          <Badge variant="destructive">deny</Badge>
        ) : (
          <span className="text-muted-foreground">—</span>
        )}
      </TableCell>
      <TableCell className="max-w-xs truncate text-muted-foreground" title={detail}>
        {detailTruncated || <span className="text-muted-foreground">—</span>}
      </TableCell>
    </TableRow>
  );
}
