import { useParams, useNavigate, Link } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { useDataset, useRefreshDataset, useDeleteDataset } from "@/api/datasets";

export default function DatasetDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  const nav = useNavigate();
  const { data: ds, isLoading } = useDataset(ws, id || null);
  const refresh = useRefreshDataset(ws, id);
  const del = useDeleteDataset(ws);

  if (isLoading) return <div className="p-4 text-muted-foreground">Loading…</div>;
  if (!ds) return <div className="p-4">Not found. <Link className="underline" to="..">Back</Link></div>;

  const source = ds.kind === "virtual" ? ds.sql : [ds.catalog, ds.schema, ds.table].filter(Boolean).join(".");

  return (
    <div className="p-4">
      <div className="mb-4 flex items-start justify-between">
        <div>
          <Link className="text-sm text-muted-foreground hover:underline" to="..">← Datasets</Link>
          <h1 className="text-lg font-semibold">{ds.name} <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-xs">{ds.kind}</span></h1>
          {ds.description && <p className="text-sm text-muted-foreground">{ds.description}</p>}
          <pre className="mt-1 max-w-2xl overflow-auto rounded bg-muted p-2 text-xs">{source}</pre>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" disabled={refresh.isPending}
            onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
            {refresh.isPending ? "Refreshing…" : "Refresh"}
          </Button>
          <Button variant="destructive"
            onClick={() => { if (confirm("Delete this dataset?")) del.mutate(id, { onSuccess: () => { toast.success("Deleted"); nav(".."); } }); }}>
            Delete
          </Button>
        </div>
      </div>

      <table className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr><th className="py-1">Column</th><th>Type</th><th>Class</th><th>Temporal</th></tr>
        </thead>
        <tbody>
          {ds.columns.map((c) => (
            <tr key={c.column_name} className="border-t">
              <td className="py-1 font-mono">{c.column_name}</td>
              <td className="font-mono text-muted-foreground">{c.data_type}</td>
              <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{c.type_generic}</span></td>
              <td>{c.is_temporal ? "✓" : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
