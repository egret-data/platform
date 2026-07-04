import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Ban } from "lucide-react";

import {
  type ClusterInfo,
  type MonitorQuery,
  getCluster,
  killQuery,
  listQueries,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
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
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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

function truncate(s: string, max: number): string {
  const flat = s.replace(/\s+/g, " ").trim();
  return flat.length > max ? flat.slice(0, max) + "…" : flat;
}

/**
 * Control Plane → Engine → Monitor tab.
 *
 * A live (3s-polled) view of the coordinator's queries + cluster stats, served
 * by egret proxying the Trino REST API. Supports client-side state/text filter
 * and killing a running query. The per-query detail panel + stage visualizer is
 * Task 7 — a `selectedQid` seam is left here for it to consume.
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

  // ── Task 7 seam: the selected query id (detail panel consumes this later). ──
  const [selectedQid, setSelectedQid] = useState<string | null>(null);
  void selectedQid;

  const [stateFilter, setStateFilter] = useState<string>("ALL");
  const [search, setSearch] = useState("");
  const [killTarget, setKillTarget] = useState<MonitorQuery | null>(null);

  const killMutation = useMutation<void, ApiError, string>({
    mutationFn: (qid) => killQuery(engineId, qid),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["queries", engineId] });
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
      <ClusterStrip cluster={cluster.data} />

      {unreachable && (
        <Alert variant="destructive">
          <AlertDescription>
            Engine not reachable — is a coordinator running?
          </AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
            <div className="space-y-1.5">
              <CardTitle className="text-lg">Queries</CardTitle>
              <CardDescription>
                Live queries on the coordinator, refreshed every 3s.
              </CardDescription>
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
        </CardHeader>
        <CardContent>
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
                  <TableCell
                    colSpan={9}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {queries.isLoading
                      ? "Loading…"
                      : unreachable
                        ? "—"
                        : "No queries."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((q) => (
                  <TableRow
                    key={q.id}
                    className="cursor-pointer"
                    onClick={() => setSelectedQid(q.id)}
                  >
                    <TableCell className="font-mono text-xs">
                      {truncate(q.id, 20)}
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
                          onClick={(e) => {
                            e.stopPropagation();
                            setKillTarget(q);
                          }}
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
        </CardContent>
      </Card>

      <AlertDialog
        open={killTarget !== null}
        onOpenChange={(o) => {
          if (!o) setKillTarget(null);
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
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
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

/** A compact strip of cluster-wide counters + node count. */
function ClusterStrip({ cluster }: { cluster: ClusterInfo | undefined }) {
  const stats: { label: string; value: number }[] = [
    { label: "Running", value: cluster?.running ?? 0 },
    { label: "Queued", value: cluster?.queued ?? 0 },
    { label: "Blocked", value: cluster?.blocked ?? 0 },
    { label: "Nodes", value: cluster?.nodes.length ?? 0 },
  ];
  return (
    <Card>
      <CardContent className="grid grid-cols-2 gap-4 py-4 sm:grid-cols-4">
        {stats.map((s) => (
          <div key={s.label} className="space-y-0.5">
            <p className="text-xs uppercase tracking-wide text-muted-foreground">
              {s.label}
            </p>
            <p className="text-2xl font-semibold tabular-nums">{s.value}</p>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}
