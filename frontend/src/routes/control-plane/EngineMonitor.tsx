import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Ban } from "lucide-react";

import {
  type ClusterInfo,
  type MonitorQuery,
  type MonitorQueryDetail,
  getCluster,
  getQuery,
  killQuery,
  listQueries,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import StageTree from "./StageTree";
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
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
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

type BadgeVariant =
  | "default"
  | "secondary"
  | "destructive"
  | "success"
  | "warning"
  | "processing"
  | "outline";

/** Distinct colors per Trino query state. */
function stateVariant(state: string): BadgeVariant {
  switch (state.toUpperCase()) {
    case "RUNNING":
      return "processing";
    case "QUEUED":
    case "PLANNING":
    case "STARTING":
      return "warning";
    case "FINISHED":
      return "success";
    case "FAILED":
      return "destructive";
    default:
      return "secondary";
  }
}

const STATE_FILTERS = [
  "ALL",
  "RUNNING",
  "QUEUED",
  "FINISHED",
  "FAILED",
] as const;

/** A running/queued query can still be killed. */
function isKillable(state: string): boolean {
  const s = state.toUpperCase();
  return s === "RUNNING" || s === "QUEUED" || s === "PLANNING" || s === "STARTING";
}

/** Live (non-terminal) states — poll the detail while true, stop when terminal. */
function isRunning(state: string): boolean {
  const s = state.toUpperCase();
  return s === "RUNNING" || s === "QUEUED" || s === "PLANNING" || s === "STARTING";
}

function truncate(s: string, max: number): string {
  const flat = s.replace(/\s+/g, " ").trim();
  return flat.length > max ? flat.slice(0, max) + "…" : flat;
}

/**
 * Control Plane → Engine → Monitor tab.
 *
 * A live (3s-polled) view of the coordinator's queries + cluster stats, served
 * by egret proxying the Trino REST API. Supports client-side state/text filter,
 * killing a running query, and a per-query detail panel (with a stage tree)
 * opened from the query-id cell.
 */
export default function EngineMonitor({ engineId }: { engineId: string }) {
  const queryClient = useQueryClient();

  const cluster = useQuery<ClusterInfo, ApiError>({
    queryKey: ["cluster", engineId],
    queryFn: () => getCluster(engineId),
    refetchInterval: 3000,
    retry: false,
  });

  const queries = useQuery<MonitorQuery[], ApiError>({
    queryKey: ["queries", engineId],
    queryFn: () => listQueries(engineId),
    refetchInterval: 3000,
    retry: false,
  });

  // ── The selected query id drives the detail panel. ──
  // Lives in the URL (`?qid=`) so an open query is deep-linkable and survives a
  // refresh. Merged alongside EngineDetail's `?tab=`. Default preserved: none.
  const [sp, setSp] = useSearchParams();
  const selectedQid = sp.get("qid");
  const setSelectedQid = (qid: string | null) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        if (qid) n.set("qid", qid);
        else n.delete("qid");
        return n;
      },
      { replace: true },
    );

  const detail = useQuery<MonitorQueryDetail, ApiError>({
    queryKey: ["query", engineId, selectedQid],
    queryFn: () => getQuery(engineId, selectedQid!),
    enabled: selectedQid !== null,
    // Poll while the query is live; stop once it reaches a terminal state.
    refetchInterval: (q) => {
      const d = q.state.data;
      return d && isRunning(d.state) ? 2000 : false;
    },
    retry: false,
  });

  const [stateFilter, setStateFilter] = useState<string>("ALL");
  const [search, setSearch] = useState("");
  const [killTarget, setKillTarget] = useState<MonitorQuery | null>(null);

  const killMutation = useMutation<void, ApiError, string>({
    mutationFn: (qid) => killQuery(engineId, qid),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["queries", engineId] });
      // Also refresh the per-query detail so an open detail dialog reflects the
      // kill instead of showing a stale RUNNING state.
      queryClient.invalidateQueries({ queryKey: ["query", engineId] });
      setKillTarget(null);
    },
  });

  const rows = useMemo(() => {
    const all = queries.data ?? [];
    const needle = search.trim().toLowerCase();
    return all.filter((q) => {
      if (stateFilter !== "ALL" && q.state.toUpperCase() !== stateFilter) {
        return false;
      }
      if (needle) {
        return (
          q.user.toLowerCase().includes(needle) ||
          q.query.toLowerCase().includes(needle)
        );
      }
      return true;
    });
  }, [queries.data, stateFilter, search]);

  // On a coordinator error the backend returns 502 — surface a friendly hint.
  const unreachable = queries.isError || cluster.isError;

  return (
    <div className="space-y-5">
      <ClusterStrip cluster={cluster.data} unreachable={unreachable} />

      {unreachable && (
        <Alert variant="destructive">
          <AlertDescription>
            Engine not reachable — is a coordinator running?
          </AlertDescription>
        </Alert>
      )}

      <section className="space-y-3">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div className="space-y-1">
            <h2 className="text-base font-semibold">Queries</h2>
            <p className="text-sm text-muted-foreground">
              Live queries on the coordinator, refreshed every 3s.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Select value={stateFilter} onValueChange={setStateFilter}>
              <SelectTrigger className="w-[140px]" aria-label="State filter">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {STATE_FILTERS.map((s) => (
                  <SelectItem key={s} value={s}>
                    {s === "ALL" ? "All states" : s}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search user or SQL…"
              className="w-[220px]"
              aria-label="Search queries"
            />
          </div>
        </div>
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Query</TableHead>
                <TableHead>User</TableHead>
                <TableHead>State</TableHead>
                <TableHead>SQL</TableHead>
                <TableHead className="w-[120px]">Progress</TableHead>
                <TableHead className="text-right">Elapsed</TableHead>
                <TableHead className="text-right">Memory</TableHead>
                <TableHead className="text-right">CPU</TableHead>
                <TableHead className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell colSpan={9} className="py-6">
                    {queries.isLoading ? (
                      <ListSkeleton rows={6} cols={5} />
                    ) : (
                      <p className="py-4 text-center text-muted-foreground">
                        {unreachable
                          ? "Coordinator unreachable — no live queries."
                          : "No queries."}
                      </p>
                    )}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((q) => (
                  <TableRow key={q.id} className="hover:bg-muted/50">
                    <TableCell className="font-mono text-xs">
                      <button
                        type="button"
                        title={q.id}
                        className="text-left hover:underline focus-visible:underline focus-visible:outline-none"
                        onClick={() => setSelectedQid(q.id)}
                      >
                        {truncate(q.id, 20)}
                      </button>
                    </TableCell>
                    <TableCell>{q.user}</TableCell>
                    <TableCell>
                      <Badge variant={stateVariant(q.state)}>{q.state}</Badge>
                    </TableCell>
                    <TableCell
                      className="max-w-[320px] truncate font-mono text-xs text-muted-foreground"
                      title={q.query}
                    >
                      {truncate(q.query, 80)}
                    </TableCell>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <Progress
                          value={Math.min(100, Math.max(0, q.progressPercent))}
                          className="h-2"
                        />
                        <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
                          {Math.round(q.progressPercent)}%
                        </span>
                      </div>
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {q.elapsed}
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {q.memory}
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {q.cpu}
                    </TableCell>
                    <TableCell className="text-right">
                      {isKillable(q.state) && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-destructive hover:text-destructive"
                          onClick={() => setKillTarget(q)}
                        >
                          <Ban className="h-4 w-4" />
                          Kill
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      </section>

      <Dialog
        open={selectedQid !== null}
        onOpenChange={(o) => {
          if (!o) setSelectedQid(null);
        }}
      >
        {/* Height cap and scrolling now come from the primitive, which scrolls an inner
            body rather than the content — the close button no longer scrolls away. */}
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <span className="font-mono text-sm">{selectedQid}</span>
              {detail.data && (
                <Badge variant={stateVariant(detail.data.state)}>
                  {detail.data.state}
                </Badge>
              )}
            </DialogTitle>
            <DialogDescription>Query detail, refreshed while running.</DialogDescription>
          </DialogHeader>

          {detail.isLoading ? (
            <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              Loading…
            </div>
          ) : detail.isError || !detail.data ? (
            <Alert variant="destructive">
              <AlertDescription>Could not load query detail.</AlertDescription>
            </Alert>
          ) : (
            <div className="space-y-4">
              {detail.data.errorMessage && (
                <Alert variant="destructive">
                  <AlertDescription className="whitespace-pre-wrap font-mono text-xs">
                    {detail.data.errorMessage}
                  </AlertDescription>
                </Alert>
              )}

              <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                {[
                  { label: "User", value: detail.data.user },
                  { label: "Elapsed", value: detail.data.elapsed },
                  { label: "CPU", value: detail.data.cpu },
                  { label: "Memory", value: detail.data.memory },
                ].map((s) => (
                  <div key={s.label} className="space-y-0.5">
                    <p className="text-xs uppercase tracking-wide text-muted-foreground">
                      {s.label}
                    </p>
                    <p className="font-mono text-sm">{s.value}</p>
                  </div>
                ))}
              </div>

              <div className="space-y-1.5">
                <div className="flex items-center gap-2">
                  <Progress
                    value={Math.min(100, Math.max(0, detail.data.progressPercent))}
                    className="h-2"
                  />
                  <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
                    {Math.round(detail.data.progressPercent)}%
                  </span>
                </div>
              </div>

              <div className="space-y-1.5">
                <p className="text-xs uppercase tracking-wide text-muted-foreground">
                  SQL
                </p>
                <pre className="max-h-64 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
                  {detail.data.query}
                </pre>
              </div>

              {detail.data.outputStage != null && (
                <div className="space-y-1.5">
                  <p className="text-xs uppercase tracking-wide text-muted-foreground">
                    Stages
                  </p>
                  {/* Trino outputStage is raw `unknown` JSON — StageTree (any) guards traversal. */}
                  <StageTree stage={detail.data.outputStage} />
                </div>
              )}

              {isKillable(detail.data.state) && (
                <div className="flex justify-end">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-destructive hover:text-destructive"
                    onClick={() => detail.data && setKillTarget(detail.data)}
                  >
                    <Ban className="h-4 w-4" />
                    Kill query
                  </Button>
                </div>
              )}
            </div>
          )}
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={killTarget !== null}
        onOpenChange={(o) => {
          if (!o) {
            setKillTarget(null);
            killMutation.reset();
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Kill this query?</AlertDialogTitle>
            <AlertDialogDescription>
              The query{" "}
              <span className="font-mono">{killTarget?.id}</span> will be
              cancelled on the coordinator. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {killMutation.isError && (
            <p className="text-sm text-destructive">
              {killMutation.error?.message ?? "Failed to kill the query."}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={(e) => {
                e.preventDefault();
                if (killTarget) killMutation.mutate(killTarget.id);
              }}
              disabled={killMutation.isPending}
            >
              {killMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Kill query
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** A compact strip of cluster-wide counters + node count. Shows em-dashes when
 *  the coordinator is unreachable so it doesn't read as a healthy idle cluster,
 *  and for the node count when the coordinator would not list its nodes. */
function ClusterStrip({
  cluster,
  unreachable,
}: {
  cluster: ClusterInfo | undefined;
  unreachable?: boolean;
}) {
  const v = (n: number | undefined) => (unreachable ? "—" : (n ?? 0));
  return (
    <SummaryStrip
      items={[
        { label: "running", value: v(cluster?.running) },
        { label: "queued", value: v(cluster?.queued) },
        { label: "blocked", value: v(cluster?.blocked) },
        {
          label: "nodes",
          value: cluster?.nodes === null ? "—" : v(cluster?.nodes.length),
        },
      ]}
    />
  );
}
