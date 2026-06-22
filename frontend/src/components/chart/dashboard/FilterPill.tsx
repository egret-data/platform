import { useState } from "react";
import { Calendar, Hash, Tag, Type, X } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { isBlank, summarize } from "./params";

const RELATIVE = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

function iconFor(p: DashboardParameter) {
  if (p.type === "date") return Calendar;
  if (p.type === "number" || p.type === "id") return Hash;
  if (p.type === "category") return Tag;
  return Type;
}

/** Cleared value for a parameter: [] for multi-value, "" otherwise. */
function clearedValue(p: DashboardParameter): unknown {
  return p.type === "category" || (p.type === "number" && p.operator === "between") ||
    (p.type === "date" && p.operator === "range") ? [] : "";
}

function hasValue(value: unknown): boolean {
  if (Array.isArray(value)) return value.filter((v) => !isBlank(v)).length > 0;
  return !isBlank(value);
}

interface Props {
  param: DashboardParameter;
  value: unknown;
  options: string[];
  onChange: (v: unknown) => void;
}

export function FilterPill({ param, value, options, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const Icon = iconFor(param);
  const summary = summarize(param, value);
  const set = hasValue(value);
  return (
    <div className={`inline-flex items-center rounded-full border text-sm transition-colors ${set ? "border-primary/40 bg-primary/5" : "bg-background hover:bg-accent/40"}`}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button type="button" aria-label={param.name} className="flex items-center gap-1.5 rounded-full px-3 py-1.5">
            <Icon className="h-3.5 w-3.5 text-muted-foreground" />
            <span>{param.name}{param.required ? " *" : ""}{set && summary ? <span className="text-muted-foreground">: {summary}</span> : null}</span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 p-2">
          <FilterControl param={param} value={value} options={options} onChange={onChange} />
        </PopoverContent>
      </Popover>
      {set && (
        <button type="button" aria-label={`clear ${param.name}`} className="pr-2.5 text-muted-foreground hover:text-foreground"
          onClick={() => onChange(clearedValue(param))}>
          <X className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function FilterControl({ param, value, options, onChange }: Props) {
  const [search, setSearch] = useState("");
  const input = "h-8 w-full rounded border bg-background px-2 text-sm";

  if (param.type === "category" || (param.type === "id" && options.length > 0)) {
    const multi = param.type === "category";
    const selected = Array.isArray(value) ? (value as string[]) : isBlank(value) ? [] : [String(value)];
    const shown = options.filter((o) => o.toLowerCase().includes(search.toLowerCase()));
    const toggle = (o: string) => {
      if (!multi) { onChange(o); return; }
      onChange(selected.includes(o) ? selected.filter((x) => x !== o) : [...selected, o]);
    };
    return (
      <div className="flex flex-col gap-1">
        <Input aria-label={`search ${param.name}`} className={input} placeholder="Search…" value={search} onChange={(e) => setSearch(e.target.value)} />
        <div className="max-h-52 overflow-auto">
          {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No values</p>
            : shown.map((o) => (
              <label key={o} aria-label={`option ${o}`} className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 text-sm hover:bg-accent/40"
                onClick={() => { if (!multi) toggle(o); }}>
                {multi
                  ? <Checkbox checked={selected.includes(o)} onCheckedChange={() => toggle(o)} />
                  : <span className={`h-3 w-3 rounded-full border ${selected.includes(o) ? "bg-primary" : ""}`} />}
                <span>{o}</span>
              </label>
            ))}
        </div>
        <button type="button" className="self-start text-xs text-muted-foreground hover:text-foreground" onClick={() => onChange(multi ? [] : "")}>Clear</button>
      </div>
    );
  }

  if (param.type === "id") {
    return <Input aria-label={`${param.name} value`} className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  if (param.type === "number" && param.operator === "between") {
    const [min, max] = Array.isArray(value) ? (value as (number | null)[]) : [null, null];
    const num = (s: string) => (s === "" ? null : Number(s));
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} min`} type="number" className={input} value={min ?? ""} onChange={(e) => onChange([num(e.target.value), max ?? null])} />
        <span className="text-muted-foreground">–</span>
        <input aria-label={`${param.name} max`} type="number" className={input} value={max ?? ""} onChange={(e) => onChange([min ?? null, num(e.target.value)])} />
      </div>
    );
  }
  if (param.type === "number") {
    return <input aria-label={`${param.name} value`} type="number" className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value === "" ? "" : Number(e.target.value))} />;
  }

  if (param.type === "date" && param.operator === "range") {
    const [start, end] = Array.isArray(value) ? (value as string[]) : ["", ""];
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} min`} type="date" className={input} value={start ?? ""} onChange={(e) => onChange([e.target.value, end ?? ""])} />
        <span className="text-muted-foreground">–</span>
        <input aria-label={`${param.name} max`} type="date" className={input} value={end ?? ""} onChange={(e) => onChange([start ?? "", e.target.value])} />
      </div>
    );
  }
  if (param.type === "date" && param.operator === "relative") {
    return (
      <div className="flex flex-col">
        {RELATIVE.map((o) => (
          <button key={o.v} type="button" aria-label={o.label} className={`rounded px-2 py-1 text-left text-sm hover:bg-accent/40 ${value === o.v ? "font-medium text-primary" : ""}`} onClick={() => onChange(o.v)}>{o.label}</button>
        ))}
      </div>
    );
  }
  if (param.type === "date") {
    return <input aria-label={`${param.name} value`} type="date" className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  return <Input aria-label={`${param.name} value`} className={input} value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
}
