import { useEffect, useState } from "react";
import { Calendar, Hash, MapPin, Tag, ToggleLeft, Type, X } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { isBlank, summarize } from "./params";
import { DateFilterPicker } from "./DateFilterPicker";

const RELATIVE = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

function iconFor(p: DashboardParameter) {
  if (p.type === "date") return Calendar;
  if (p.type === "number" || p.type === "id") return Hash;
  if (p.type === "category") return Tag;
  if (p.type === "location") return MapPin;
  if (p.type === "boolean") return ToggleLeft;
  return Type;
}

/** Cleared value for a parameter: [] for multi-value, "" otherwise. */
function clearedValue(p: DashboardParameter): unknown {
  return p.type === "category" || p.type === "location" ||
    (p.type === "number" && p.operator === "between") ||
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
    <div className={`inline-flex h-8 items-center rounded-sm border px-2 text-sm transition-colors ${set ? "border-primary/40 bg-secondary" : "bg-background hover:bg-accent/40"}`}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button type="button" aria-label={param.name} className="flex h-full items-center gap-1.5">
            <Icon className="h-3.5 w-3.5 text-muted-foreground" />
            <span>{param.name}{param.required ? " *" : ""}{set && summary ? <span className="text-muted-foreground">: {summary}</span> : null}</span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 p-2">
          <FilterControl param={param} value={value} options={options} onChange={onChange} />
        </PopoverContent>
      </Popover>
      {set && (
        <button type="button" aria-label={`clear ${param.name}`} className="pl-1.5 text-muted-foreground hover:text-foreground"
          onClick={() => onChange(clearedValue(param))}>
          <X className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function FilterControl({ param, value, options, onChange }: Props) {
  const [search, setSearch] = useState("");
  const [qy, setQy] = useState(() => {
    const [y = "", q = ""] = String(value ?? "").split("-");
    return { y, q };
  });
  // Re-seed local quarter-year state when the external value changes (e.g. the
  // clear-X button or a programmatic default), so local state never goes stale.
  useEffect(() => {
    const [y2 = "", q2 = ""] = String(value ?? "").split("-");
    setQy({ y: y2, q: q2 });
  }, [value]);
  const input = "h-8 w-full rounded border bg-background px-2 text-sm";
  const widget = param.widget ?? "dropdown";
  const listType = param.type === "category" || param.type === "location";

  // boolean: 3-state Any / True / False
  if (param.type === "boolean") {
    return (
      <select aria-label={`${param.name} value`} className={input}
        value={value === true || value === "true" ? "true" : value === false || value === "false" ? "false" : ""}
        onChange={(e) => onChange(e.target.value)}>
        <option value="">Any</option>
        <option value="true">True</option>
        <option value="false">False</option>
      </select>
    );
  }

  // all-options: full relative/specific date picker
  if (param.type === "date" && param.operator === "all-options") {
    return <DateFilterPicker value={value as never} onChange={(v) => onChange(v)} />;
  }

  // month-year
  if (param.type === "date" && param.operator === "month-year") {
    return <input aria-label={`${param.name} value`} type="month" className={input}
      value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  // quarter-year: year input + quarter select, encoded as "YYYY-Q"
  if (param.type === "date" && param.operator === "quarter-year") {
    const emit = (y: string, quarter: string) => {
      setQy({ y, q: quarter });
      onChange(y ? `${y}-${quarter || "1"}` : "");
    };
    return (
      <div className="flex items-center gap-1">
        <input aria-label={`${param.name} year`} type="number" placeholder="Year" className={input}
          value={qy.y} onChange={(e) => emit(e.target.value, qy.q)} />
        <select aria-label={`${param.name} quarter`} className={input} value={qy.q || "1"}
          onChange={(e) => emit(qy.y || String(new Date().getUTCFullYear()), e.target.value)}>
          <option value="1">Q1</option><option value="2">Q2</option><option value="3">Q3</option><option value="4">Q4</option>
        </select>
      </div>
    );
  }

  // widget=input on a list-ish type → single free-text value
  if (widget === "input" && (listType || param.type === "id" || param.type === "text")) {
    return <Input aria-label={`${param.name} value`} className={input}
      value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} />;
  }

  // dropdown / search list for category, location, and id-with-options
  if (listType || (param.type === "id" && options.length > 0)) {
    const multi = listType;
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
