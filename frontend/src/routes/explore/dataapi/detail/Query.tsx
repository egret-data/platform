import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useEndpointDetail } from "../EndpointDetail";
import { PreviewBlock } from "./PreviewBlock";

/** Placeholder — Task 3 moves the database picker and `ParamEditor` in here. */
export default function Query() {
  const { form, setForm } = useEndpointDetail();
  return (
    <div className="space-y-6">
      <div className="space-y-1.5">
        <Label htmlFor="q-sql">SQL</Label>
        <Textarea
          id="q-sql"
          className="font-mono text-xs"
          rows={8}
          value={form.sql}
          onChange={(e) => setForm((f) => ({ ...f, sql: e.target.value }))}
        />
      </div>
      <PreviewBlock />
    </div>
  );
}
