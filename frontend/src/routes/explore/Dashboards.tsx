import { useMemo, useState } from "react";
import { Search, Star } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations, type DashboardSummary } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { DashboardActionsMenu } from "@/components/chart/dashboard/DashboardActionsMenu";
import { BulkToggle } from "@/components/explore/BulkBar";
import { useBulkSelect } from "@/hooks/useBulkSelect";

type SortKey = "name" | "card_count" | "updated_at";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useDashboards(ws, { trashed: showTrash });
  const { create, remove, duplicate, trash, restore, addBookmark, removeBookmark } = useDashboardMutations(ws);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DashboardSummary | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((d) => {
      if (onlyBookmarked && !d.bookmarked) return false;
      if (!q) return true;
      return d.name.toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "card_count") cmp = a.card_count - b.card_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  const bulk = useBulkSelect(rows);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    try { const d = await create.mutateAsync({ name: name.trim() }); setOpen(false); setName(""); nav(`/explore/${ws}/dashboards/${d.id}`); }
    catch (e) { toast.error(String(e)); }
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  async function doBulkTrash() {
    for (const id of bulk.selectedIds) { try { await trash.mutateAsync(id); } catch (e) { toast.error(String(e)); } }
    toast.success("Moved to Trash"); bulk.clear();
  }

  const hasData = !!data && data.length > 0;

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Dashboards"}</h1>
        <div className="flex gap-2">
          {!showTrash && (
            <Button variant={onlyBookmarked ? "default" : "outline"} onClick={() => setOnlyBookmarked((v) => !v)}>★ Bookmarks</Button>
          )}
          {!showTrash && hasData && (
            <BulkToggle active={bulk.bulkMode} onToggle={() => bulk.setBulkMode(!bulk.bulkMode)} />
          )}
          <Button variant={showTrash ? "default" : "outline"} onClick={() => { const next = !showTrash; setShowTrash(next); if (next) { setOnlyBookmarked(false); bulk.setBulkMode(false); } }}>
            {showTrash ? "← Dashboards" : "Trash"}
          </Button>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild><Button>+ Dashboard</Button></DialogTrigger>
            <DialogContent>
              <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
              <div className="flex gap-2 py-2">
                <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
                <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search dashboards…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            {bulk.bulkMode && bulk.someSelected && !showTrash && (
              <Button variant="destructive" size="sm" onClick={doBulkTrash}>
                Move {bulk.selectedIds.length} to Trash
              </Button>
            )}
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  {bulk.bulkMode && !showTrash && <th className="w-8 py-1"><Checkbox checked={bulk.allSelected} onCheckedChange={bulk.toggleAll} aria-label="Select all" /></th>}
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="card_count" label="Cards" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((d) => (
                  <tr key={d.id} className="border-t hover:bg-accent">
                    {bulk.bulkMode && !showTrash && <td className="py-1"><Checkbox checked={bulk.selected.has(d.id)} onCheckedChange={() => bulk.toggle(d.id)} aria-label={`Select ${d.name}`} /></td>}
                    <td className="py-1">
                      <span className="inline-flex items-center gap-1">
                        {!showTrash && (
                          <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
                            onClick={() => guard((d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id))}>
                            <Star className={`h-4 w-4 ${d.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`} />
                          </Button>
                        )}
                        <Link className="font-medium hover:underline" to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</Link>
                      </span>
                    </td>
                    <td className="text-muted-foreground">{d.card_count}</td>
                    <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
                    <td>
                      <DashboardActionsMenu
                        ws={ws} dash={d} trashed={showTrash}
                        onDuplicate={(x) => guard(duplicate.mutateAsync(x.id).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
                        onTrash={(x) => guard(trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")))}
                        onRestore={(x) => guard(restore.mutateAsync(x.id))}
                        onDeletePermanent={setPendingDelete}
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked dashboards." : "No dashboards match the current filters."}</p>
          )}
        </>
      ) : (
        <p className="text-muted-foreground">{showTrash ? "Trash is empty." : "No dashboards yet."}</p>
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(remove.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />
    </div>
  );
}
