import { useMemo, useState } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";

import { listUsers } from "@/api/access";
import { dataApiActivityCalls, useDataApiListEndpoints } from "@/api/generated/data-api/data-api";
import type { ActivityCallDto, ActivityRange } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

/** Backend default page size (`crates/egret-dataapi/src/activity.rs`'s
 * `calls()` doc: "Default 50, maximum 200"). Sent explicitly rather than
 * relying on the server default, and used as the "was this page full"
 * signal — the endpoint returns a bare array, not a `{ rows, nextCursor }`
 * envelope, so a page shorter than the limit is the only way to tell there
 * is nothing further to load. */
const PAGE_SIZE = 50;

/**
 * Call-list section of Activity: filters, the cursor-paginated table, and
 * the "Include trial runs" toggle. Split out of Activity.tsx because the
 * filter bar + pagination + trial-labelling logic is its own tier — see the
 * Task 9 report for the line-count rationale.
 *
 * Pagination is `useInfiniteQuery`, not a `useState` accumulator kept in
 * sync by a `useEffect` (the shape `AuditLog.tsx`'s "Load more" uses).
 * `eslint.config.js:35` enables `react-hooks/set-state-in-effect` across
 * this repo, and AuditLog's two effects already spend two of the lint
 * baseline's 178 allowed warnings; copying that shape here would add two
 * more, which the task brief was explicit about not doing. `useInfiniteQuery`
 * gets the same behavior — "any filter change resets pagination" — for
 * free: every filter is part of the query key below, so changing one is a
 * brand-new query to react-query, which starts back at page one on its own.
 */
export default function ActivityCallList({ ws, range }: { ws: string; range: ActivityRange }) {
  const [endpointId, setEndpointId] = useState("all");
  const [statusCode, setStatusCode] = useState("");
  const [errorCode, setErrorCode] = useState("");
  const [requestId, setRequestId] = useState("");
  // Off by default (spec §4.8 / brief): a trial is not consumer traffic, so
  // an operator must opt in to see it mixed into the list.
  const [includeTrials, setIncludeTrials] = useState(false);

  const endpointParam = endpointId === "all" ? undefined : endpointId;
  const trimmedStatus = statusCode.trim();
  const parsedStatus = trimmedStatus ? Number(trimmedStatus) : undefined;
  const statusParam = parsedStatus !== undefined && Number.isFinite(parsedStatus) ? parsedStatus : undefined;
  const errorCodeParam = errorCode.trim() || undefined;
  const requestIdParam = requestId.trim() || undefined;

  const endpointsQuery = useDataApiListEndpoints(ws);
  const endpoints = endpointsQuery.data?.status === 200 ? endpointsQuery.data.data : [];

  // Trial rows are identified by `actor_user_id`, and resolving it to an
  // email is the same problem AuditLog.tsx solves for its actor column.
  const usersQuery = useQuery({
    queryKey: ["data-api", "activity", "users"],
    queryFn: listUsers,
    retry: false,
  });
  const userEmailById = useMemo(
    () => new Map((usersQuery.data ?? []).map((u) => [u.id, u.email])),
    [usersQuery.data],
  );

  const callsQuery = useInfiniteQuery({
    queryKey: [
      "data-api",
      "activity",
      "calls",
      ws,
      range,
      endpointParam,
      statusParam,
      errorCodeParam,
      requestIdParam,
      includeTrials,
    ],
    queryFn: async ({ pageParam }) => {
      const res = await dataApiActivityCalls(ws, {
        range,
        endpoint_id: endpointParam,
        status_code: statusParam,
        error_code: errorCodeParam,
        request_id: requestIdParam,
        include_trials: includeTrials,
        before: pageParam,
        limit: PAGE_SIZE,
      });
      // `apiFetch` throws on any non-2xx (see mutator.ts), so this branch is
      // only ever reached to satisfy the generated response union's
      // TypeScript narrowing — same pattern every other Data API list page
      // uses on its own hook's `.data?.status === 200` check.
      if (res.status !== 200) throw new Error("Could not load the call list.");
      return res.data;
    },
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) =>
      lastPage.length >= PAGE_SIZE ? lastPage[lastPage.length - 1]?.ts : undefined,
    retry: false,
  });

  // Flatten + dedupe pages during render: a derived value read straight off
  // the query result every render, not a second copy kept in state.
  const entries = useMemo(() => {
    const seen = new Set<string>();
    const merged: ActivityCallDto[] = [];
    for (const page of callsQuery.data?.pages ?? []) {
      for (const c of page) {
        if (!seen.has(c.id)) {
          seen.add(c.id);
          merged.push(c);
        }
      }
    }
    return merged;
  }, [callsQuery.data]);

  const isInitialLoading = callsQuery.isLoading;
  const errorMessage =
    (callsQuery.error as { message?: string } | null)?.message ?? "Could not load the call list.";

  return (
    <div className="space-y-3">
      <h2 className="text-sm font-semibold">Calls</h2>

      <div className="flex flex-wrap items-end gap-3">
        <div className="space-y-1.5">
          <Label htmlFor="activity-endpoint">Endpoint</Label>
          <Select value={endpointId} onValueChange={setEndpointId}>
            <SelectTrigger id="activity-endpoint" className="w-48" aria-label="Endpoint">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All endpoints</SelectItem>
              {endpoints.map((e) => (
                <SelectItem key={e.id} value={e.id}>
                  {e.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="activity-status">Status code</Label>
          <Input
            id="activity-status"
            value={statusCode}
            onChange={(e) => setStatusCode(e.target.value)}
            placeholder="e.g. 404"
            className="w-28"
            inputMode="numeric"
          />
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="activity-error-code">Error code</Label>
          <Input
            id="activity-error-code"
            value={errorCode}
            onChange={(e) => setErrorCode(e.target.value)}
            placeholder="e.g. rate_limited"
            className="w-40"
          />
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="activity-request-id">Request ID</Label>
          <Input
            id="activity-request-id"
            value={requestId}
            onChange={(e) => setRequestId(e.target.value)}
            placeholder="Look up a request out of an error body"
            className="w-64"
          />
        </div>

        <div className="flex items-center gap-2 pb-2">
          <Checkbox
            id="activity-include-trials"
            checked={includeTrials}
            onCheckedChange={(c) => setIncludeTrials(c === true)}
          />
          <Label htmlFor="activity-include-trials" className="font-normal">
            Include trial runs
          </Label>
        </div>
      </div>

      {callsQuery.isError ? (
        <Alert variant="destructive">
          <AlertDescription>{errorMessage}</AlertDescription>
        </Alert>
      ) : isInitialLoading ? (
        <ListSkeleton cols={7} />
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Time</TableHead>
                <TableHead>Endpoint</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="text-right">Duration</TableHead>
                <TableHead>Cache</TableHead>
                <TableHead>Client</TableHead>
                <TableHead>Request ID</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map((c) => (
                <CallRow key={c.id} call={c} userEmailById={userEmailById} />
              ))}
              {entries.length === 0 && (
                <TableRow className="hover:bg-transparent">
                  <TableCell colSpan={7} className="py-8 text-center text-muted-foreground">
                    No calls in range.
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
            Showing <span className="tabular-nums">{entries.length}</span> call
            {entries.length === 1 ? "" : "s"}
            {callsQuery.hasNextPage ? " so far" : ""}
          </span>
          {callsQuery.hasNextPage && (
            <Button
              variant="outline"
              size="sm"
              disabled={callsQuery.isFetchingNextPage}
              onClick={() => void callsQuery.fetchNextPage()}
            >
              {callsQuery.isFetchingNextPage ? "Loading…" : "Load more"}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function CallRow({
  call,
  userEmailById,
}: {
  call: ActivityCallDto;
  userEmailById: Map<string, string>;
}) {
  const isTrial = call.actor_user_id != null;
  const endpointLabel = call.endpoint_id
    ? (call.endpoint_name ?? call.endpoint_id)
    : [call.request_method, call.request_path].filter(Boolean).join(" ") || "—";

  const statusTone =
    call.status_code >= 200 && call.status_code < 300
      ? "bg-success/15 text-success"
      : call.status_code >= 400
        ? "bg-destructive/15 text-destructive"
        : "bg-muted text-muted-foreground";

  return (
    <TableRow>
      <TableCell className="whitespace-nowrap tabular-nums text-muted-foreground">
        <RelativeTime iso={call.ts} />
      </TableCell>
      <TableCell>{endpointLabel}</TableCell>
      <TableCell>
        <span
          className={`inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium tabular-nums ${statusTone}`}
        >
          {call.status_code}
        </span>
        {call.error_code && (
          <span className="ml-1.5 text-xs text-muted-foreground">{call.error_code}</span>
        )}
      </TableCell>
      <TableCell className="text-right tabular-nums">{call.duration_ms} ms</TableCell>
      <TableCell>{call.cache_hit ? "Hit" : "Miss"}</TableCell>
      <TableCell>
        {isTrial ? (
          // Trials are labelled with the ACTOR, never `client_id` — a
          // trial's `sub` is the reserved marker `trial:<service_account_id>`,
          // which client_id carries instead of a real client identity.
          <span className="inline-flex items-center gap-1.5 text-xs">
            <Badge variant="outline">Trial</Badge>
            <span className="text-muted-foreground">
              {(call.actor_user_id && userEmailById.get(call.actor_user_id)) ??
                call.actor_user_id?.slice(0, 8)}
            </span>
          </span>
        ) : (
          <span className="font-mono text-xs text-muted-foreground">{call.client_id ?? "—"}</span>
        )}
      </TableCell>
      <TableCell className="font-mono text-xs">{call.request_id}</TableCell>
    </TableRow>
  );
}
