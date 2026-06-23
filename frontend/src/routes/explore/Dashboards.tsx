import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations, type DashboardSummary } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useDashboards(ws, { trashed: showTrash });
  const { create, remove, duplicate, trash, restore } = useDashboardMutations(ws);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [pendingDelete, setPendingDelete] = useState<DashboardSummary | null>(null);

  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    try {
      const d = await create.mutateAsync({ name: name.trim() });
      setOpen(false); setName("");
      nav(`/explore/${ws}/dashboards/${d.id}`);
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Dashboards"}</h1>
        <div className="flex gap-2">
          <Button size="sm" variant={showTrash ? "default" : "outline"} onClick={() => setShowTrash((v) => !v)}>
            {showTrash ? "← Dashboards" : "Trash"}
          </Button>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild><Button size="sm">+ Dashboard</Button></DialogTrigger>
            <DialogContent>
              <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
              <div className="flex gap-2 py-2">
                <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
                <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>
      {isLoading ? (
        <p className="text-muted-foreground">Loading…</p>
      ) : !data?.length ? (
        <p className="text-muted-foreground">{showTrash ? "Trash is empty." : "No dashboards yet."}</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-muted-foreground">
            <tr><th className="py-1 pr-3">Name</th><th className="pr-3">Cards</th><th className="pr-3">Updated</th><th /></tr>
          </thead>
          <tbody>
            {data.map((d) => (
              <tr key={d.id} className="border-t">
                <td className="py-1 pr-3"><Link className="hover:underline" to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</Link></td>
                <td className="pr-3 text-muted-foreground">{d.card_count}</td>
                <td className="pr-3 text-muted-foreground">{new Date(d.updated_at).toLocaleString()}</td>
                <td className="flex gap-1">
                  {showTrash ? (
                    <>
                      <Button size="sm" variant="ghost" aria-label={`restore ${d.name}`}
                        onClick={async () => { try { await restore.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>Restore</Button>
                      <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" aria-label={`delete ${d.name} permanently`}
                        onClick={() => setPendingDelete(d)}>Delete permanently</Button>
                    </>
                  ) : (
                    <>
                      <Button size="sm" variant="ghost" aria-label={`duplicate ${d.name}`}
                        onClick={async () => { try { const c = await duplicate.mutateAsync(d.id); nav(`/explore/${ws}/dashboards/${c.id}`); } catch (e) { toast.error(String(e)); } }}>Duplicate</Button>
                      <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" aria-label={`move ${d.name} to trash`}
                        onClick={async () => { try { await trash.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>Delete</Button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => {
          if (!pendingDelete) return;
          try { await remove.mutateAsync(pendingDelete.id); } catch (e) { toast.error(String(e)); }
          setPendingDelete(null);
        }}
      />
    </div>
  );
}
