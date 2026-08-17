import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useDatabases } from "@/api/databases";
import { useEndpointDetail } from "../EndpointDetail";
import { ParamEditor } from "./ParamEditor";
import { PreviewBlock } from "./PreviewBlock";

/**
 * Everything that decides what the query RETURNS: the database it runs against,
 * the SQL, the parameters it declares, and a preview of the rows.
 *
 * **Vertical — editor above, results below — following SQL Lab, not
 * ChartExplore's left/right split.** ChartExplore owns the full page width; this
 * section sits inside the detail route under the shell's `max-w-3xl`, and a
 * right-hand results column would squeeze both halves.
 *
 * Parameters appear twice here on purpose. `ParamEditor` holds the DECLARATION
 * (name, type, required, default, allowed) which is saved with the endpoint;
 * `PreviewBlock` holds a TEST VALUE per declared parameter, which is not. Putting
 * a test value in the declaration's row would invite the reader to think it was
 * stored.
 */
export default function Query() {
  const { ws, form, setForm } = useEndpointDetail();
  const { data: databases } = useDatabases(ws);

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Query</h2>
        <p className="text-sm text-muted-foreground">
          The query this endpoint runs, and the parameters callers may pass it.
        </p>
      </div>

      <div className="space-y-4">
        <div className="space-y-1.5">
          <Label htmlFor="q-db">Database</Label>
          <Select value={form.database_id} onValueChange={(v) => setForm((f) => ({ ...f, database_id: v }))}>
            <SelectTrigger id="q-db" aria-label="Database">
              <SelectValue placeholder="Choose a database" />
            </SelectTrigger>
            <SelectContent>
              {(databases ?? []).map((d) => (
                <SelectItem key={d.id} value={d.id}>
                  {d.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="q-sql">SQL</Label>
          <Textarea
            id="q-sql"
            className="font-mono text-xs"
            rows={12}
            value={form.sql}
            onChange={(e) => setForm((f) => ({ ...f, sql: e.target.value }))}
          />
        </div>
      </div>

      <ParamEditor params={form.params} onChange={(params) => setForm((f) => ({ ...f, params }))} />

      <PreviewBlock />
    </div>
  );
}
