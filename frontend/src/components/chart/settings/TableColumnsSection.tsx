import type { TableState } from "@/api/charts";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

const PAGE_SIZES = [25, 50, 100];

export function TableColumnsSection({
  columns, value, onChange,
}: {
  columns: ResultColumn[];
  value: TableState;
  onChange: (next: TableState) => void;
}) {
  const hidden = new Set(value.hiddenColumns ?? []);
  const pinnedLeft = value.pinnedLeft ?? 0;
  const pagination = value.pagination ?? { enabled: false, pageSize: 25 };

  function toggleHidden(name: string) {
    const next = new Set(hidden);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({ ...value, hiddenColumns: [...next] });
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="text-xs font-medium uppercase text-muted-foreground">Visible columns</div>
        <div className="flex flex-col gap-1">
          {columns.map((c) => (
            <div key={c.name} className="flex items-center gap-2">
              <Checkbox id={`tcol-${c.name}`} checked={!hidden.has(c.name)} onCheckedChange={() => toggleHidden(c.name)} />
              <Label htmlFor={`tcol-${c.name}`} className="cursor-pointer text-sm font-normal">{c.name}</Label>
            </div>
          ))}
        </div>
      </div>

      <div className="space-y-1">
        <Label htmlFor="freeze-left" className="text-xs font-medium uppercase text-muted-foreground">Freeze first columns</Label>
        <Input
          id="freeze-left" type="number" min={0} max={columns.length} value={pinnedLeft}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ ...value, pinnedLeft: Number.isNaN(n) ? 0 : Math.max(0, Math.min(n, columns.length)) });
          }}
          className="h-8 text-sm"
        />
      </div>

      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <Checkbox id="paginate" checked={pagination.enabled}
            onCheckedChange={(v) => onChange({ ...value, pagination: { ...pagination, enabled: !!v } })} />
          <Label htmlFor="paginate" className="cursor-pointer text-sm font-normal">Paginate</Label>
        </div>
        {pagination.enabled && (
          <div className="space-y-1">
            <Label className="text-xs font-medium uppercase text-muted-foreground">Page size</Label>
            <Select value={String(pagination.pageSize)} onValueChange={(v) => onChange({ ...value, pagination: { ...pagination, pageSize: Number(v) } })}>
              <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
              <SelectContent>
                {PAGE_SIZES.map((s) => <SelectItem key={s} value={String(s)}>{s}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>
    </div>
  );
}
