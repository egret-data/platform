import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const { data, isLoading } = useDashboards(ws);
  const { create, remove } = useDashboardMutations(ws);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

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
        <h1 className="text-lg font-semibold">Dashboards</h1>
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
      {isLoading ? (
        <p className="text-muted-foreground">Loading…</p>
      ) : !data?.length ? (
        <p className="text-muted-foreground">No dashboards yet.</p>
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
                <td>
                  <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive"
                    onClick={async () => { try { await remove.mutateAsync(d.id); } catch (e) { toast.error(String(e)); } }}>
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
