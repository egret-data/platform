import { useState } from "react";
import { toast } from "sonner";
import { Pencil, Trash2 } from "lucide-react";
import {
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle,
  AlertDialogDescription, AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
} from "@/components/ui/alert-dialog";
import {
  useSavedQueries, useUpdateSavedQuery, useDeleteSavedQuery,
  type SavedQuery, type DatabaseListItem,
} from "@/api/sqllab";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

function dbName(databases: DatabaseListItem[] | undefined, id: string | null): string {
  if (!id) return "—";
  return databases?.find((d) => d.id === id)?.name ?? `${id.slice(0, 8)}…`;
}

export function SavedTab({ ws, databases, onOpen }: {
  ws: string;
  databases: DatabaseListItem[] | undefined;
  onOpen: (sql: string, dbId: string | null) => void;
}) {
  const { data } = useSavedQueries(ws);
  const update = useUpdateSavedQuery(ws);
  const del = useDeleteSavedQuery(ws);
  const [editing, setEditing] = useState<SavedQuery | null>(null);
  const [deleting, setDeleting] = useState<SavedQuery | null>(null);
  const items = data ?? [];

  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No saved queries.</div>;

  return (
    <>
      <div className="divide-y">
        {items.map((q) => (
          <div key={q.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
            <button className="min-w-0 flex-1 text-left" onClick={() => onOpen(q.sql, q.database_id)}>
              <div className="flex items-center gap-2 text-sm">
                <span className="font-semibold">{q.name}</span>
                <span className="text-xs text-muted-foreground">{dbName(databases, q.database_id)}</span>
                <span className="text-xs text-muted-foreground" title={fullTimestamp(q.updated_at)}>updated {timeAgo(q.updated_at)}</span>
              </div>
              <div className="truncate text-xs text-muted-foreground" title={q.description || q.sql}>{q.description || q.sql}</div>
            </button>
            <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
              <button className="rounded p-1 hover:bg-background" title="Edit" aria-label="Edit" onClick={() => setEditing(q)}><Pencil className="h-4 w-4" /></button>
              <button className="rounded p-1 text-red-600 hover:bg-background" title="Delete" aria-label="Delete" onClick={() => setDeleting(q)}><Trash2 className="h-4 w-4" /></button>
            </div>
          </div>
        ))}
      </div>

      <SaveQueryDialog
        open={editing != null}
        onOpenChange={(o) => { if (!o) setEditing(null); }}
        mode="edit"
        initialName={editing?.name ?? ""}
        initialDescription={editing?.description ?? ""}
        onSave={async (name, description) => {
          if (!editing) return;
          await update.mutateAsync({ id: editing.id, database_id: editing.database_id, name, sql: editing.sql, description });
          toast.success("Updated");
          setEditing(null);
        }}
      />

      <AlertDialog open={deleting != null} onOpenChange={(o) => { if (!o) setDeleting(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete saved query?</AlertDialogTitle>
            <AlertDialogDescription>“{deleting?.name}” will be permanently deleted.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => { if (deleting) del.mutate(deleting.id); setDeleting(null); }}
            >Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
