import { useState } from "react";
import { toast } from "sonner";
import { Braces, Pencil, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  useCreateSnippet, useDeleteSnippet, useSnippets, useUpdateSnippet, type SqlSnippet,
} from "@/api/sqllab";

type Editing = { id?: string; name: string; body: string; description: string } | null;

export function SnippetsTab({ ws, onInsert, currentSql }: {
  ws: string; onInsert: (body: string) => void; currentSql: string;
}) {
  const { data } = useSnippets(ws);
  const create = useCreateSnippet(ws);
  const update = useUpdateSnippet(ws);
  const del = useDeleteSnippet(ws);
  const [editing, setEditing] = useState<Editing>(null);
  const [deleting, setDeleting] = useState<SqlSnippet | null>(null);

  const items = data ?? [];
  const guard = (p: Promise<unknown>, ok: string) => p.then(() => toast.success(ok), (e) => toast.error(String(e)));

  async function save() {
    if (!editing) return;
    const body = { name: editing.name.trim(), body: editing.body, description: editing.description.trim() };
    if (!body.name || !body.body.trim()) { toast.error("Name and body required"); return; }
    if (editing.id) await guard(update.mutateAsync({ id: editing.id, ...body }), "Snippet updated");
    else await guard(create.mutateAsync(body), "Snippet saved");
    setEditing(null);
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-3 py-1.5">
        <span className="text-xs font-medium text-muted-foreground">{items.length} snippet{items.length === 1 ? "" : "s"}</span>
        <Button size="sm" variant="outline" className="h-7 gap-1"
          onClick={() => setEditing({ name: "", body: currentSql, description: "" })}>
          <Plus className="h-3.5 w-3.5" /> New snippet
        </Button>
      </div>

      {items.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
          <Braces className="h-7 w-7 opacity-40" />
          <div>No snippets yet.</div>
          <div className="text-xs">Save reusable SQL and insert it into any query.</div>
        </div>
      ) : (
        <div className="flex-1 divide-y overflow-auto">
          {items.map((s: SqlSnippet) => (
            <div key={s.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
              <button className="min-w-0 flex-1 text-left" onClick={() => onInsert(s.body)} title="Insert into editor">
                <div className="flex items-center gap-2 text-sm">
                  <Braces className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span className="font-medium">{s.name}</span>
                  {s.description && <span className="truncate text-xs text-muted-foreground">{s.description}</span>}
                </div>
                <div className="truncate pl-5 font-mono text-xs text-muted-foreground" title={s.body}>{s.body}</div>
              </button>
              <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
                <button className="rounded p-1 hover:bg-background" title="Edit" aria-label={`Edit ${s.name}`}
                  onClick={() => setEditing({ id: s.id, name: s.name, body: s.body, description: s.description })}>
                  <Pencil className="h-4 w-4" />
                </button>
                <button className="rounded p-1 text-red-600 hover:bg-background" title="Delete" aria-label={`Delete ${s.name}`}
                  onClick={() => setDeleting(s)}>
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Confirmed first, as deleting a saved query is (KI-091). */}
      <AlertDialog open={deleting != null} onOpenChange={(o) => { if (!o) setDeleting(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete snippet?</AlertDialogTitle>
            <AlertDialogDescription>“{deleting?.name}” will be permanently deleted for everyone in this workspace.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => { if (deleting) guard(del.mutateAsync(deleting.id), "Snippet deleted"); setDeleting(null); }}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog open={editing != null} onOpenChange={(o) => { if (!o) setEditing(null); }}>
        <DialogContent className="max-w-lg">
          <DialogHeader><DialogTitle>{editing?.id ? "Edit snippet" : "New snippet"}</DialogTitle></DialogHeader>
          {editing && (
            <div className="space-y-3">
              <Input autoFocus placeholder="Name" value={editing.name}
                onChange={(e) => setEditing({ ...editing, name: e.target.value })} />
              <Textarea placeholder="SQL body" value={editing.body} rows={6} className="font-mono text-xs"
                onChange={(e) => setEditing({ ...editing, body: e.target.value })} />
              <Input placeholder="Description (optional)" value={editing.description}
                onChange={(e) => setEditing({ ...editing, description: e.target.value })} />
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditing(null)}>Cancel</Button>
            <Button onClick={save} disabled={!editing?.name.trim() || !editing?.body.trim()}>Save</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
