import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useDatabases } from "@/api/sqllab";
import { useDatasets, useDataset, useDeleteDataset, type DatasetSummary } from "@/api/datasets";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import { DatasetActionsMenu } from "@/components/dataset/DatasetActionsMenu";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

type SortKey = "name" | "column_count" | "updated_at";

function sourceOf(d: DatasetSummary): string {
  return d.kind === "virtual" ? "SQL" : [d.catalog, d.schema, d.table].filter(Boolean).join(".");
}

export default function Datasets() {
  const ws = useParams().workspace!;
  const { data, isLoading } = useDatasets(ws);
  const { data: databases } = useDatabases(ws);
  const del = useDeleteDataset(ws);

  const [createOpen, setCreateOpen] = useState(false);
  const [bulkMode, setBulkMode] = useState(false);
  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [dbFilter, setDbFilter] = useState("all");
  const [sort, setSort] = useState<{ key: SortKey; dir: "asc" | "desc" }>({ key: "updated_at", dir: "desc" });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [deleteTarget, setDeleteTarget] = useState<DatasetSummary | null>(null);
  const [bulkOpen, setBulkOpen] = useState(false);
  const [dupId, setDupId] = useState<string | null>(null);
  const dup = useDataset(ws, dupId);

  const dbName = (id: string) => {
    const d = databases?.find((x) => x.id === id);
    return d ? `${d.name} (${d.engine})` : id.slice(0, 8);
  };

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((d) => {
      if (typeFilter !== "all" && d.kind !== typeFilter) return false;
      if (dbFilter !== "all" && d.database_id !== dbFilter) return false;
      if (!q) return true;
      const hay = [d.name, d.description, d.schema ?? "", d.table ?? "", d.catalog ?? "", dbName(d.database_id)]
        .join(" ").toLowerCase();
      return hay.includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "column_count") cmp = a.column_count - b.column_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, databases, query, typeFilter, dbFilter, sort]);

  const hasData = !!data && data.length > 0;
  const allSelected = rows.length > 0 && rows.every((d) => selected.has(d.id));
  const someSelected = rows.some((d) => selected.has(d.id));

  function toggle(id: string) {
    setSelected((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  }
  function toggleAll() {
    setSelected((s) => {
      if (rows.every((d) => s.has(d.id))) { const n = new Set(s); rows.forEach((d) => n.delete(d.id)); return n; }
      const n = new Set(s); rows.forEach((d) => n.add(d.id)); return n;
    });
  }
  function setSortKey(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function doDelete(id: string) {
    await del.mutateAsync(id);
    setSelected((s) => { const n = new Set(s); n.delete(id); return n; });
  }
  async function doBulkDelete() {
    const ids = [...selected];
    try {
      await Promise.all(ids.map((id) => del.mutateAsync(id)));
      toast.success(`Deleted ${ids.length}`);
      setSelected(new Set());
    } catch (e) { toast.error(String(e)); }
    setBulkOpen(false);
  }

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Datasets</h1>
        <div className="flex items-center gap-2">
          {hasData && (
            <Button variant={bulkMode ? "secondary" : "outline"}
              onClick={() => { setBulkMode((v) => !v); setSelected(new Set()); }}>
              {bulkMode ? "Cancel" : "Bulk select"}
            </Button>
          )}
          <Button onClick={() => setCreateOpen(true)}>Create</Button>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={6} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search name, schema, table…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            <Select value={typeFilter} onValueChange={setTypeFilter}>
              <SelectTrigger className="h-9 w-36"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All types</SelectItem>
                <SelectItem value="physical">Physical</SelectItem>
                <SelectItem value="virtual">Virtual</SelectItem>
              </SelectContent>
            </Select>
            <Select value={dbFilter} onValueChange={setDbFilter}>
              <SelectTrigger className="h-9 w-48"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All databases</SelectItem>
                {(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}
              </SelectContent>
            </Select>
            {bulkMode && someSelected && (
              <Button variant="destructive" size="sm" onClick={() => setBulkOpen(true)}>
                Delete {[...selected].filter((id) => rows.some((r) => r.id === id)).length}
              </Button>
            )}
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulkMode && <th className="w-8 py-1"><Checkbox checked={allSelected} onCheckedChange={toggleAll} aria-label="Select all" /></th>}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={setSortKey} />
                  <th>Kind</th>
                  <th>Database</th>
                  <th>Source</th>
                  <SortableHeader sortKey="column_count" label="Columns" current={sort} onSort={setSortKey} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={setSortKey} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((d) => (
                  <tr key={d.id} className="border-t hover:bg-accent">
                    {bulkMode && <td className="py-1"><Checkbox checked={selected.has(d.id)} onCheckedChange={() => toggle(d.id)} aria-label={`Select ${d.name}`} /></td>}
                    <td className="py-1"><Link className="font-medium hover:underline" to={d.id}>{d.name}</Link></td>
                    <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{d.kind}</span></td>
                    <td className="text-muted-foreground">{dbName(d.database_id)}</td>
                    <td className="font-mono text-xs text-muted-foreground">{sourceOf(d)}</td>
                    <td>{d.column_count}</td>
                    <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
                    <td><DatasetActionsMenu ws={ws} ds={d} onDuplicate={(x) => setDupId(x.id)} onDelete={setDeleteTarget} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">No datasets match the current filters.</p>
          )}
        </>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No datasets yet. A dataset is a named, reusable source for charts.</p>
          <Button className="mt-3" onClick={() => setCreateOpen(true)}>Create your first dataset</Button>
        </div>
      )}

      <CreateDatasetDialog ws={ws} open={createOpen} onOpenChange={setCreateOpen} />

      {dup.data && (
        <CreateDatasetDialog
          key={`dup-${dup.data.id}`}
          ws={ws}
          open
          onOpenChange={(o) => { if (!o) setDupId(null); }}
          initialTab="sql"
          initialSql={dup.data.sql ?? ""}
          initialDbId={dup.data.database_id}
          initialName={`copy of ${dup.data.name}`}
        />
      )}

      <AlertDialog open={!!deleteTarget} onOpenChange={(o) => { if (!o) setDeleteTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete dataset “{deleteTarget?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              This removes the dataset and its column snapshot. The underlying table or query is not affected. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => { if (deleteTarget) doDelete(deleteTarget.id).then(() => toast.success("Deleted")).catch((e) => toast.error(String(e))); setDeleteTarget(null); }}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={bulkOpen} onOpenChange={setBulkOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {selected.size} datasets?</AlertDialogTitle>
            <AlertDialogDescription>
              This removes the selected datasets and their column snapshots. Underlying tables and queries are not affected. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90" onClick={doBulkDelete}>
              Delete {selected.size}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
