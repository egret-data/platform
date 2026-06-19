import { Link, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useCharts, useChartMutations } from "@/api/charts";
import { Button } from "@/components/ui/button";

export default function Charts() {
  const ws = useParams().workspace!;
  const { data: charts, isLoading } = useCharts(ws);
  const { remove } = useChartMutations(ws);

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Charts</h1>
        <Button asChild size="sm"><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
      </div>
      {isLoading ? (
        <p className="text-muted-foreground">Loading…</p>
      ) : !charts?.length ? (
        <p className="text-muted-foreground">No charts yet.</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-muted-foreground">
            <tr><th className="py-1 pr-3">Name</th><th className="pr-3">Type</th><th className="pr-3">Updated</th><th /></tr>
          </thead>
          <tbody>
            {charts.map((c) => (
              <tr key={c.id} className="border-t">
                <td className="py-1 pr-3"><Link className="hover:underline" to={`/explore/${ws}/charts/${c.id}`}>{c.name}</Link></td>
                <td className="pr-3">{c.viz_type === "pivot_table" ? "Pivot" : "Table"}</td>
                <td className="pr-3 text-muted-foreground">{new Date(c.updated_at).toLocaleString()}</td>
                <td>
                  <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive"
                    onClick={async () => { try { await remove.mutateAsync(c.id); } catch (e) { toast.error(String(e)); } }}>
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
