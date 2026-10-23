import { useEffect, useState } from "react";
import { Calendar, Clock, Hash, MapPin, Tag, ToggleLeft, Type, X } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";
import { isBlank, summarize } from "./params";
import { isMultiValue } from "./multiSelect";
import { DateFilterPicker } from "./DateFilterPicker";

const RELATIVE = [
  { v: "last7", label: "Last 7 days" }, { v: "last30", label: "Last 30 days" },
  { v: "last90", label: "Last 90 days" }, { v: "last365", label: "Last 365 days" },
];

/** The boolean filter's "Any" item. A Radix item cannot carry "", which is what "Any" stores, so
 *  this sentinel stands in for it and is converted back to "" before `onChange` sees it. */
const ANY = "any";

function iconFor(p: DashboardParameter) {
  if (p.type === "date") return Calendar;
  if (p.type === "number" || p.type === "id") return Hash;
  if (p.type === "category") return Tag;
  if (p.type === "location") return MapPin;
  if (p.type === "boolean") return ToggleLeft;
  if (p.type === "temporal_unit") return Clock;
  return Type;
}

/** Cleared value for a parameter: [] where the control emits an array, "" otherwise. */
function clearedValue(p: DashboardParameter): unknown {
  const listType = p.type === "category" || p.type === "location" || p.type === "id";
  if (listType && isMultiValue(p)) return [];
  if (p.type === "number" && p.operator === "between") return [];
  if (p.type === "date" && p.operator === "range") return [];
  return "";
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
      {set && param.type !== "temporal_unit" && (
        <button type="button" aria-label={`clear ${param.name}`} className="pl-1.5 text-muted-foreground hover:text-foreground"
          onClick={() => onChange(clearedValue(param))}>
          <X className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

/** The value control for one parameter, dispatching on type and operator. Exported because the
 *  DEFAULT value is a parameter value too, and the settings panel must offer the same widget the
 *  viewer will use — the structure Metabase gets from sharing `ParameterValueWidget`. */
export function FilterControl({ param, value, options, onChange }: Props) {
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
      <Select value={value === true || value === "true" ? "true" : value === false || value === "false" ? "false" : ANY}
        onValueChange={(v) => onChange(v === ANY ? "" : v)}>
        <SelectTrigger size="sm" className="w-full" aria-label={`${param.name} value`}><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value={ANY}>Any</SelectItem>
          <SelectItem value="true">True</SelectItem>
          <SelectItem value="false">False</SelectItem>
        </SelectContent>
      </Select>
    );
  }

  if (param.type === "temporal_unit") {
    return (
      <Select value={String(value ?? param.default ?? "month")} onValueChange={(v) => onChange(v)}>
        <SelectTrigger size="sm" className="w-full" aria-label={`${param.name} value`}><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectLabel>Group by</SelectLabel>
            {TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => (
              <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
            ))}
          </SelectGroup>
          <SelectGroup>
            <SelectLabel>Extract</SelectLabel>
            {TEMPORAL_UNITS.filter((u) => u.group === "extract").map((u) => (
              <SelectItem key={u.token} value={u.token}>{u.label}</SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
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
        <Select value={qy.q || "1"} onValueChange={(v) => emit(qy.y || String(new Date().getUTCFullYear()), v)}>
          <SelectTrigger size="sm" className="w-full" aria-label={`${param.name} quarter`}><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="1">Q1</SelectItem>
            <SelectItem value="2">Q2</SelectItem>
            <SelectItem value="3">Q3</SelectItem>
            <SelectItem value="4">Q4</SelectItem>
          </SelectContent>
        </Select>
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
    // The list is rendered for the list types and for an id that has options; whether it is a
    // checkbox list or a radio list is the author's "People can pick" choice.
    const multi = isMultiValue(param);
    const selected = Array.isArray(value) ? (value as string[]) : isBlank(value) ? [] : [String(value)];
    // Dropdown lists every value and the box narrows it. Search box lists nothing until the
    // viewer types, then only the matches: the widget for a column with too many values to read
    // as a list (KI-066). The picked values stay on show so they can be unpicked.
    const isSearchBox = widget === "search";
    const query = search.trim().toLowerCase();
    const shown = isSearchBox && query === ""
      ? options.filter((o) => selected.includes(o))
      : options.filter((o) => o.toLowerCase().includes(query));
    const toggle = (o: string) => {
      if (!multi) { onChange(o); return; }
      onChange(selected.includes(o) ? selected.filter((x) => x !== o) : [...selected, o]);
    };
    return (
      <div className="flex flex-col gap-1">
        <Input aria-label={`search ${param.name}`} className={input} placeholder={isSearchBox ? "Search the values…" : "Search…"} value={search} onChange={(e) => setSearch(e.target.value)} />
        <div className="max-h-52 overflow-auto">
          {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">{isSearchBox && query === "" ? "Type to search the values" : "No values"}</p>
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
