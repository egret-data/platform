import { useState } from "react";
import { toast } from "sonner";
import { Copy, Play, Search, Trash2 } from "lucide-react";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useClearHistory, useHistory, type DatabaseListItem } from "@/api/sqllab";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";
import { rowsLabel } from "@/components/sqllab/rowsLabel";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

function dbName(databases: DatabaseListItem[] | undefined, id: string): string {
  return databases?.find((d) => d.id === id)?.name ?? `${id.slice(0, 8)}…`;
}

export function HistoryTab({ ws, databases, onLoad, onRun }: {
  ws: string;
  databases: DatabaseListItem[] | undefined;
  onLoad: (sql: string) => void;
  onRun: (sql: string, dbId: string) => void;
}) {
  const { data } = useHistory(ws);
  const clear = useClearHistory(ws);
  const items = data ?? [];
  const [q, setQ] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No queries run yet.</div>;

  async function copy(sql: string) {
    try { await navigator.clipboard.writeText(sql); toast.success("Copied"); }
    catch { toast.error("Copy failed"); }
  }

  const needle = q.trim().toLowerCase();
  const shown = needle
    ? items.filter((h) => h.sql.toLowerCase().includes(needle) || dbName(databases, h.database_id).toLowerCase().includes(needle))
    : items;

  return (
    <div>
      <div className="sticky top-0 z-10 flex items-center gap-2 border-b bg-background p-2">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search history…" aria-label="Search query history"
            className="h-7 w-full rounded-md border bg-background pl-7 pr-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring" />
        </div>
        <Button size="sm" variant="outline" className="h-7 gap-1" onClick={() => setConfirmClear(true)}>
          <Trash2 className="h-3.5 w-3.5" /> Clear history
        </Button>
      </div>
      {shown.length === 0 ? (
        <div className="p-4 text-center text-sm text-muted-foreground">No history matches “{q}”.</div>
      ) : (
      <div className="divide-y">
      {shown.map((h) => (
        <div key={h.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
          <button className="min-w-0 flex-1 text-left" onClick={() => onLoad(h.sql)}>
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <QueryStatusBadge status={h.status} />
              <span>{dbName(databases, h.database_id)}</span>
              <span>·</span>
              <span title={fullTimestamp(h.submitted_at)}>{timeAgo(h.submitted_at)}</span>
              {h.row_count != null && (<><span>·</span><span>{rowsLabel(h.row_count)}</span></>)}
            </div>
            <div className="truncate font-mono text-sm" title={h.sql}>{h.sql}</div>
          </button>
          <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
            <button className="rounded p-1 hover:bg-background" title="Copy SQL" aria-label="Copy SQL" onClick={() => copy(h.sql)}><Copy className="h-4 w-4" /></button>
            <button className="rounded p-1 hover:bg-background" title="Re-run" aria-label="Re-run" onClick={() => onRun(h.sql, h.database_id)}><Play className="h-4 w-4" /></button>
          </div>
        </div>
      ))}
      </div>
      )}
      <AlertDialog open={confirmClear} onOpenChange={setConfirmClear}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Clear your query history?</AlertDialogTitle>
            <AlertDialogDescription>
              Your queries in this workspace leave the History list. Saved queries and other people's history are not affected.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => clear.mutateAsync().then(() => toast.success("History cleared"), (e) => toast.error(String(e)))}>
              Clear history
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
