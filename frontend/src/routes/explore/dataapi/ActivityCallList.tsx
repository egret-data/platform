import { type Ref, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { Check, Copy, Inbox } from "lucide-react";
import { Link } from "react-router-dom";
import { useInfiniteQuery } from "@tanstack/react-query";

import { dataApiActivityCalls, useDataApiListEndpoints } from "@/api/generated/data-api/data-api";
import type { ActivityCallDto, ActivityRange } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListStates } from "@/components/explore/ListStates";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { CHIP_TONE } from "./chipTone";
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
/**
 * What `Activity` may ask this list to do from the outside. One command, not a
 * filter it owns: the four filter `useState`s below stay here, which is what
 * makes them survive a range switch, and this is an EVENT ("show me that
 * endpoint's calls") rather than a second copy of one of them held upstairs.
 */
export interface ActivityCallListHandle {
  /** Narrow to one endpoint and put focus on the control that did it. */
  showCallsFor: (endpointId: string) => void;
}

export default function ActivityCallList({
  ws,
  range,
  rangeIsEmpty,
  ref,
}: {
  ws: string;
  range: ActivityRange;
  /** React 19 passes `ref` as an ordinary prop — no `forwardRef` wrapper. */
  ref?: Ref<ActivityCallListHandle>;
  /** The summary has SETTLED and says this range produced no calls — the
   * unfiltered truth the filter row is gated on (§2.3). False while that query
   * is in flight, which is not the same claim: gated on `!hasCalls` instead,
   * the row would blink out of existence on every range switch. */
  rangeIsEmpty: boolean;
}) {
  const [endpointId, setEndpointId] = useState("all");
  const [statusCode, setStatusCode] = useState("");
  const [errorCode, setErrorCode] = useState("");
  const [requestId, setRequestId] = useState("");
  // Off by default (spec §4.8): a trial is not consumer traffic, so an operator
  // opts in to see it. Local state again, and it survives a range switch
  // because this component no longer unmounts on one.
  const [includeTrials, setIncludeTrials] = useState(false);

  // Focus moves to the filter, and that is the point of the command rather
  // than a flourish on it: the per-endpoint table is above the fold and this
  // list is below it, so setting the filter alone changes nothing a keyboard
  // user can perceive and scrolls nothing for a pointer user (browsers scroll
  // to focus). The Endpoint filter is also the control that UNDOES the choice,
  // so it is the right place to be standing afterwards.
  const endpointTrigger = useRef<HTMLButtonElement>(null);
  // No dependency array, deliberately, and for `PreviewBlock.tsx`'s reason: a
  // handle that is rebuilt every render can never be stale, whereas `[]` freezes
  // whatever it closed over and goes silently wrong the day someone adds a
  // non-stable capture. Everything it touches today (two setters, one ref) is
  // stable anyway, so the array would buy nothing and cost that.
  useImperativeHandle(ref, () => ({
    showCallsFor: (id: string) => {
      setEndpointId(id);
      // The Request ID filter is cleared, and ONLY that one. A request id is a
      // point lookup — it names exactly one call — so intersecting it with
      // "show me this endpoint's calls" can only ever yield that one call or
      // nothing, and a control captioned `Show calls for X` that lands on an
      // empty list is a control that lied. Status code and error code are
      // cross-cutting predicates an operator may well want to carry INTO an
      // endpoint ("show me the 500s, now just this endpoint"), so they survive;
      // if they miss, the list already says "No calls match the current
      // filters" with both inputs visible directly above it.
      setRequestId("");
      endpointTrigger.current?.focus();
    },
  }));

  const endpointParam = endpointId === "all" ? undefined : endpointId;
  const trimmedStatus = statusCode.trim();
  const parsedStatus = trimmedStatus ? Number(trimmedStatus) : undefined;
  const statusParam = parsedStatus !== undefined && Number.isFinite(parsedStatus) ? parsedStatus : undefined;
  const errorCodeParam = errorCode.trim() || undefined;
  const requestIdParam = requestId.trim() || undefined;

  const endpointsQuery = useDataApiListEndpoints(ws);
  const endpointsKnown = endpointsQuery.data?.status === 200;
  const endpoints = endpointsQuery.data?.status === 200 ? endpointsQuery.data.data : [];

  // A selected endpoint the live list does not contain. It can only get here
  // through `showCallsFor` — the summary's `LEFT JOIN dataapi.endpoints` still
  // reports a deleted endpoint's `access_log` rows, under its id with a null
  // name — and without an option to match it, `SelectValue` renders blank while
  // the list below it is narrowed: a filter in effect that the screen does not
  // admit to. Gated on `endpointsKnown` so a still-loading list cannot produce
  // a duplicate `<SelectItem value>` the moment the real one arrives.
  const unlistedEndpoint =
    endpointsKnown && endpointParam !== undefined && !endpoints.some((e) => e.id === endpointParam)
      ? endpointParam
      : undefined;

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

  // The four NARROWING controls, which is what §1.2 gates. "Include trial runs"
  // is deliberately not one of them (see the header below).
  const narrowingFiltersEngaged =
    endpointParam !== undefined ||
    statusParam !== undefined ||
    errorCodeParam !== undefined ||
    requestIdParam !== undefined;

  // §1.2: "a filter or a bulk mode over zero rows is a control that cannot do
  // anything." The two other disjuncts are what keeps that from becoming a
  // trap: rows on screen (trials, which the summary never counts) mean the
  // filters CAN act, and a filter that is engaged must stay reachable even
  // when it matches nothing — the same argument §1.2 records for the way OUT
  // of an empty trash.
  const showFilters = !rangeIsEmpty || entries.length > 0 || narrowingFiltersEngaged;

  return (
    // Testid, not a heading lookup: this list's empty sentence and the page's
    // own empty state are one word apart on purpose, so every assertion about
    // this list's states has to be scoped to it — and in the error state there
    // is no table here to scope by.
    <div data-testid="activity-call-list" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">Calls</h2>

        {/* NOT gated on `hasCalls`, on §1.2's view-switch rule: this does not
            narrow the rows in front of the reader, it selects a different
            POPULATION the summary above never counted at all. An empty
            consumer-call list says nothing about whether there were trials —
            exactly what an empty endpoint list says about the trash. Gated, it
            would be removed precisely when it is the only control left. */}
        <div className="flex items-center gap-2">
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

      {showFilters && (
        <div className="flex flex-wrap items-end gap-3">
          <div className="space-y-1.5">
            <Label htmlFor="activity-endpoint">Endpoint</Label>
            <Select value={endpointId} onValueChange={setEndpointId}>
              <SelectTrigger
                id="activity-endpoint"
                ref={endpointTrigger}
                className="w-48"
                aria-label="Endpoint"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All endpoints</SelectItem>
                {endpoints.map((e) => (
                  <SelectItem key={e.id} value={e.id}>
                    {e.name}
                  </SelectItem>
                ))}
                {/* Labelled with the id, which is exactly what the per-endpoint
                    table shows for the same row when the name is gone. */}
                {unlistedEndpoint && <SelectItem value={unlistedEndpoint}>{unlistedEndpoint}</SelectItem>}
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
        </div>
      )}

      {/* `isError` before `hasData`, and empty never stands in for a failure
          (§2.1/§2.2) — the branch order is `ListStates`', not hand-rolled. */}
      <ListStates
        isLoading={callsQuery.isLoading}
        isError={callsQuery.isError}
        error={callsQuery.error}
        errorText="Could not load the call list."
        hasData={entries.length > 0}
        skeletonCols={7}
        empty={
          // Two situations, two sentences (§2.3) — and the predicate is what
          // the reader can see engaged, nothing else. "Include trial runs" is
          // NOT in it: it is a view switch over a population the summary never
          // counted, so an empty result with only that on is an empty range,
          // not a mismatch. Counting it here had the page reporting "No calls
          // match the current filters" with no filter set.
          narrowingFiltersEngaged ? (
            <p className="py-8 text-center text-muted-foreground">
              No calls match the current filters.
            </p>
          ) : (
            // The page's ONE empty state. It lives here because this section is
            // the only one mounted in every state: the tiles and the
            // per-endpoint table above simply do not render when there is
            // nothing to put in them.
            <EmptyState
              variant="dashed"
              icon={Inbox}
              title="No calls in this range"
              description="Nothing has called this workspace's endpoints in the selected range. Try-it runs are not counted above; include them beside Calls to see them here."
              action={
                <Button asChild variant="outline">
                  <Link to={`/explore/${ws}/data-api/endpoints`}>View endpoints</Link>
                </Button>
              }
            />
          )
        }
      >
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
                <CallRow key={c.id} call={c} />
              ))}
            </TableBody>
          </Table>
        </div>
      </ListStates>

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

function CallRow({ call }: { call: ActivityCallDto }) {
  const isTrial = call.actor_user_id != null;
  const endpointLabel = call.endpoint_id
    ? (call.endpoint_name ?? call.endpoint_id)
    : [call.request_method, call.request_path].filter(Boolean).join(" ") || "—";

  // The chip is a scan aid; the STATUS CODE is the information, and it is
  // already three digits of text — `200` and `500` differ without a hue, which
  // is why the ink can be flattened to `--foreground` without costing the
  // column a channel. It has to be: `text-success` on this wash measures
  // 3.72:1 and `text-destructive` 3.68:1 against WCAG 1.4.3's 4.5:1 for 12px
  // text (`chipTone.ts`).
  const statusTone =
    call.status_code >= 200 && call.status_code < 300
      ? CHIP_TONE.success
      : call.status_code >= 400
        ? CHIP_TONE.destructive
        : CHIP_TONE.neutral;

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
              {call.actor_name ?? call.actor_user_id?.slice(0, 8)}
            </span>
          </span>
        ) : (
          <span className="inline-flex items-center gap-1">
            <span className="font-mono text-xs text-muted-foreground">{call.client_id ?? "—"}</span>
            {/* No button when there is no id: a "Copy" over `—` copies nothing.
                A trial row does not reach here at all — its `client_id` is the
                reserved `trial:<service_account_id>` marker, not a client
                identity, and handing that to someone as if it were one is how
                a support thread goes wrong. */}
            {call.client_id && (
              <CopyIdButton value={call.client_id} label={`Copy client id ${call.client_id}`} />
            )}
          </span>
        )}
      </TableCell>
      <TableCell className="font-mono text-xs">
        <span className="inline-flex items-center gap-1">
          <span>{call.request_id}</span>
          <CopyIdButton value={call.request_id} label={`Copy request id ${call.request_id}`} />
        </span>
      </TableCell>
    </TableRow>
  );
}

/**
 * The `Clients.tsx` copy affordance, at the same size and with the same
 * "Copy"/"Copied" idiom (`TryIt.tsx`, `ClientCreate.tsx` and
 * each carry their own copy of it).
 *
 * It belongs on this table more than on most: the Request ID filter three rows
 * up is captioned "Look up a request out of an error body", so pasting a
 * request id INTO this page is the documented workflow — and until now the
 * page showed the ids and offered no way to lift one off the screen. A call
 * row had no button and no link in it at all.
 *
 * The label carries the VALUE rather than the word "Copy": fifty rows would
 * otherwise be fifty controls with one accessible name, which is unusable by
 * voice and by screen reader alike.
 */
function CopyIdButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const revert = useRef<number | undefined>(undefined);
  // The rows under this component are paginated and DO unmount — "Load more"
  // replaces nothing, but a filter change throws the whole list away — so the
  // 1.5s revert has to be cancellable or it fires into a dead component.
  useEffect(() => () => window.clearTimeout(revert.current), []);
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="h-6 w-6 shrink-0 p-0 text-muted-foreground"
      aria-label={label}
      onClick={() => {
        // `writeText` REJECTS — denied permission, a document that is not
        // focused, no clipboard API at all over plain http. The four existing
        // copies of this idiom in the repo (`Clients.tsx:483`, `TryIt.tsx`,
        // `ClientCreate.tsx`) all `void` the promise and
        // flip to "Copied" regardless, which tells the user the id is on their
        // clipboard when it is not — and the id is the whole point of the
        // button. This fifth copy waits for the promise instead. The other four
        // are recorded in the task report rather than edited here.
        void navigator.clipboard
          ?.writeText(value)
          .then(() => {
            setCopied(true);
            window.clearTimeout(revert.current);
            revert.current = window.setTimeout(() => setCopied(false), 1500);
          })
          .catch(() => {
            // Nothing claimed. Silence is not ideal, but a button that stays
            // un-ticked is at least not a lie, and there is no toast host on
            // this page to put a better answer in.
          });
      }}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </Button>
  );
}
