import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { useDatasets } from "@/api/datasets";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";

export default function Datasets() {
  const ws = useParams().workspace!;
  const { data, isLoading } = useDatasets(ws);
  const [open, setOpen] = useState(false);

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Datasets</h1>
        <Button onClick={() => setOpen(true)}>Create</Button>
      </div>
      {isLoading ? <p className="text-muted-foreground">Loading…</p>
        : (data && data.length > 0) ? (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr><th className="py-1">Name</th><th>Kind</th><th>Columns</th><th>Updated</th></tr>
            </thead>
            <tbody>
              {data.map((d) => (
                <tr key={d.id} className="border-t hover:bg-accent">
                  <td className="py-1"><Link className="font-medium hover:underline" to={d.id}>{d.name}</Link></td>
                  <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{d.kind}</span></td>
                  <td>{d.column_count}</td>
                  <td className="text-muted-foreground">{new Date(d.updated_at).toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No datasets yet. A dataset is a named, reusable source for charts.</p>
            <Button className="mt-3" onClick={() => setOpen(true)}>Create your first dataset</Button>
          </div>
        )}
      <CreateDatasetDialog ws={ws} open={open} onOpenChange={setOpen} />
    </div>
  );
}
