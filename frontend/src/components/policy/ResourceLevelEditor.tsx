import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { type ResourceLevel, resourceLookup } from "@/api/policies";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";

interface Props {
  label: string;
  level: "catalog" | "schema" | "table" | "column";
  service: string;
  parents: { catalog?: string; schema?: string; table?: string };
  value: ResourceLevel;
  onChange: (v: ResourceLevel) => void;
}

export function ResourceLevelEditor({ label, level, service, parents, value, onChange }: Props) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const sug = useQuery<string[]>({
    queryKey: ["resource-lookup", service, level, parents, q],
    queryFn: () => resourceLookup(service, level, parents, q),
    enabled: open,
    retry: false,
  });
  const add = (v: string) => {
    const t = v.trim();
    if (t && !value.values.includes(t)) onChange({ ...value, values: [...value.values, t] });
    setQ("");
  };
  const remove = (v: string) => onChange({ ...value, values: value.values.filter((x) => x !== v) });
  const suggestions = (sug.data ?? []).filter((s) => !value.values.includes(s));

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between">
        <label className="text-sm font-medium">{label}</label>
        <div className="flex items-center gap-3 text-xs text-muted-foreground">
          <span className="flex items-center gap-1">
            <Switch aria-label={`Exclude ${label}`} checked={!!value.isExcludes}
              onCheckedChange={(c) => onChange({ ...value, isExcludes: c })} />
            Exclude
          </span>
          <span className="flex items-center gap-1">
            <Switch aria-label={`Recursive ${label}`} checked={!!value.isRecursive}
              onCheckedChange={(c) => onChange({ ...value, isRecursive: c })} />
            Recursive
          </span>
        </div>
      </div>
      <div className="flex flex-wrap gap-1">
        {value.values.map((v) => (
          <Badge key={v} variant="secondary" className="gap-1">
            {v}
            <button type="button" aria-label={`remove ${v}`} onClick={() => remove(v)}>
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Input placeholder="value or * (Enter to add)" value={q} onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); add(q); } }} />
        </PopoverTrigger>
        <PopoverContent align="start" className="max-h-56 w-64 overflow-y-auto p-1" onOpenAutoFocus={(e) => e.preventDefault()}>
          {suggestions.map((s) => (
            <button key={s} type="button" onClick={() => add(s)}
              className="block w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent">
              {s}
            </button>
          ))}
          {suggestions.length === 0 && (
            <div className="px-2 py-1.5 text-sm text-muted-foreground">Type a value or * and press Enter.</div>
          )}
        </PopoverContent>
      </Popover>
    </div>
  );
}
