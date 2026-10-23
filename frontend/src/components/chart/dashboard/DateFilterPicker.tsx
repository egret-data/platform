import { useEffect, useState } from "react";
import type { DateAllOptionsValue, DateUnit } from "./params";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

type RelVal = Extract<DateAllOptionsValue, { mode: "relative" }>;
type SpecVal = Extract<DateAllOptionsValue, { mode: "specific" }>;

interface Props {
  value: DateAllOptionsValue | undefined;
  onChange: (v: DateAllOptionsValue) => void;
}

const input = "h-8 rounded border bg-background px-2 text-sm";

export function DateFilterPicker({ value, onChange }: Props) {
  const [draft, setDraft] = useState<DateAllOptionsValue | undefined>(value);
  // Explicit user tab override; cleared whenever the value prop changes so the
  // displayed tab follows the (re-synced) draft's mode.
  const [tabOverride, setTabOverride] = useState<"relative" | "specific" | null>(null);
  useEffect(() => {
    setDraft(value);
    setTabOverride(null);
  }, [value]);

  const tab: "relative" | "specific" =
    tabOverride ?? (draft?.mode === "specific" ? "specific" : "relative");
  const setTab = setTabOverride;

  const rel: RelVal =
    draft?.mode === "relative" ? draft : { mode: "relative", dir: "past", n: 7, unit: "day" };
  const spec: SpecVal =
    draft?.mode === "specific" ? draft : { mode: "specific", op: "on", from: "" };

  const emit = (next: DateAllOptionsValue) => {
    setDraft(next);
    onChange(next);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-1">
        <button
          type="button"
          aria-label="Relative"
          className={`rounded px-2 py-1 text-sm ${tab === "relative" ? "bg-accent" : ""}`}
          onClick={() => setTab("relative")}
        >
          Relative
        </button>
        <button
          type="button"
          aria-label="Specific"
          className={`rounded px-2 py-1 text-sm ${tab === "specific" ? "bg-accent" : ""}`}
          onClick={() => setTab("specific")}
        >
          Specific
        </button>
      </div>
      {tab === "relative" ? (
        <div className="flex items-center gap-1">
          <Select value={rel.dir} onValueChange={(v) => emit({ ...rel, dir: v as RelVal["dir"] })}>
            <SelectTrigger size="sm" aria-label="relative dir"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="past">Past</SelectItem>
              <SelectItem value="next">Next</SelectItem>
              <SelectItem value="current">Current</SelectItem>
            </SelectContent>
          </Select>
          {rel.dir !== "current" && (
            <input
              aria-label="relative n"
              type="number"
              min={1}
              className={`${input} w-16`}
              value={rel.n}
              onChange={(e) => emit({ ...rel, n: Math.max(1, Number(e.target.value) || 1) })}
            />
          )}
          <Select value={rel.unit} onValueChange={(v) => emit({ ...rel, unit: v as DateUnit })}>
            <SelectTrigger size="sm" aria-label="relative unit"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="day">days</SelectItem>
              <SelectItem value="week">weeks</SelectItem>
              <SelectItem value="month">months</SelectItem>
              <SelectItem value="quarter">quarters</SelectItem>
              <SelectItem value="year">years</SelectItem>
            </SelectContent>
          </Select>
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <Select value={spec.op} onValueChange={(v) => emit({ ...spec, op: v as SpecVal["op"] })}>
            <SelectTrigger size="sm" className="w-full" aria-label="specific op"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="on">On</SelectItem>
              <SelectItem value="before">Before</SelectItem>
              <SelectItem value="after">After</SelectItem>
              <SelectItem value="between">Between</SelectItem>
            </SelectContent>
          </Select>
          <input
            aria-label="specific from"
            type="date"
            className={input}
            value={spec.from}
            onChange={(e) => emit({ ...spec, from: e.target.value })}
          />
          {spec.op === "between" && (
            <input
              aria-label="specific to"
              type="date"
              className={input}
              value={spec.to ?? ""}
              onChange={(e) => emit({ ...spec, to: e.target.value })}
            />
          )}
        </div>
      )}
    </div>
  );
}
