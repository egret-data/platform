import { toast } from "sonner";
import { Copy, Play } from "lucide-react";
import { useHistory, type DatabaseListItem } from "@/api/sqllab";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";
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
  const items = data ?? [];
  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No queries run yet.</div>;

  async function copy(sql: string) {
    try { await navigator.clipboard.writeText(sql); toast.success("Copied"); }
    catch { toast.error("Copy failed"); }
  }

  return (
    <div className="divide-y">
      {items.map((h) => (
        <div key={h.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
          <button className="min-w-0 flex-1 text-left" onClick={() => onLoad(h.sql)}>
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <QueryStatusBadge status={h.status} />
              <span>{dbName(databases, h.database_id)}</span>
              <span>·</span>
              <span title={fullTimestamp(h.submitted_at)}>{timeAgo(h.submitted_at)}</span>
              {h.row_count != null && (<><span>·</span><span>{h.row_count} rows</span></>)}
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
  );
}
