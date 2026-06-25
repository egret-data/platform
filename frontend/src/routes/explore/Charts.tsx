import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useCharts, useChartMutations, type Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ChartActionsMenu } from "@/components/chart/ChartActionsMenu";

type SortKey = "name" | "viz_type" | "updated_at";
const vizMeta = (v: string) => CHART_TYPES.find((t) => t.key === v);
const vizLabel = (v: string) => vizMeta(v)?.name ?? v;

export default function Charts() {
  const ws = useParams().workspace!;
  const { data: charts, isLoading } = useCharts(ws);
  const { remove } = useChartMutations(ws);

  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [deleteTarget, setDeleteTarget] = useState<Chart | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (charts ?? []).filter((c) => {
      if (typeFilter !== "all" && c.viz_type !== typeFilter) return false;
      if (!q) return true;
      return [c.name, vizLabel(c.viz_type)].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "viz_type") cmp = vizLabel(a.viz_type).localeCompare(vizLabel(b.viz_type));
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [charts, query, typeFilter, sort]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function doDelete(c: Chart) {
    try { await remove.mutateAsync(c.id); toast.success("Deleted"); }
    catch (e) { toast.error(String(e)); }
  }

  const hasData = !!charts && charts.length > 0;
  const presentTypes = CHART_TYPES.filter((t) => (charts ?? []).some((c) => c.viz_type === t.key));

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Charts</h1>
        <Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search charts…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            <Select value={typeFilter} onValueChange={setTypeFilter}>
              <SelectTrigger className="h-9 w-44"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All types</SelectItem>
                {presentTypes.map((t) => <SelectItem key={t.key} value={t.key}>{t.name}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="viz_type" label="Type" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => {
                  const Icon = vizMeta(c.viz_type)?.icon;
                  return (
                    <tr key={c.id} className="border-t hover:bg-accent">
                      <td className="py-1">
                        <Link className="inline-flex items-center gap-2 font-medium hover:underline" to={`/explore/${ws}/charts/${c.id}`}>
                          {Icon && <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />}
                          {c.name}
                        </Link>
                      </td>
                      <td className="text-muted-foreground">{vizLabel(c.viz_type)}</td>
                      <td className="text-muted-foreground"><RelativeTime iso={c.updated_at} /></td>
                      <td><ChartActionsMenu ws={ws} chart={c} onDelete={setDeleteTarget} /></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">No charts match the current filters.</p>
          )}
        </>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No charts yet. Build a chart from a dataset.</p>
          <Button asChild className="mt-3"><Link to={`/explore/${ws}/charts/new`}>Create your first chart</Link></Button>
        </div>
      )}

      <AlertDialog open={!!deleteTarget} onOpenChange={(o) => { if (!o) setDeleteTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete chart “{deleteTarget?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>This permanently deletes the chart. This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => { if (deleteTarget) doDelete(deleteTarget); setDeleteTarget(null); }}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
