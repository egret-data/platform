import { useEffect, useState } from "react";
import type { DateAllOptionsValue, DateUnit } from "./params";

type RelVal = Extract<DateAllOptionsValue, { mode: "relative" }>;
type SpecVal = Extract<DateAllOptionsValue, { mode: "specific" }>;

interface Props {
  value: DateAllOptionsValue | undefined;
  onChange: (v: DateAllOptionsValue) => void;
}

const input = "h-8 rounded border bg-background px-2 text-sm";

export function DateFilterPicker({ value, onChange }: Props) {
  const [draft, setDraft] = useState<DateAllOptionsValue | undefined>(value);
  useEffect(() => setDraft(value), [value]);

  const [tab, setTab] = useState<"relative" | "specific">(
    draft?.mode === "specific" ? "specific" : "relative",
  );

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
          <select
            aria-label="relative dir"
            className={input}
            value={rel.dir}
            onChange={(e) => emit({ ...rel, dir: e.target.value as RelVal["dir"] })}
          >
            <option value="past">Past</option>
            <option value="next">Next</option>
            <option value="current">Current</option>
          </select>
          {rel.dir !== "current" && (
            <input
              aria-label="relative n"
              type="number"
              min={1}
              className={`${input} w-16`}
              value={rel.n}
              onChange={(e) => emit({ ...rel, n: Number(e.target.value) || 0 })}
            />
          )}
          <select
            aria-label="relative unit"
            className={input}
            value={rel.unit}
            onChange={(e) => emit({ ...rel, unit: e.target.value as DateUnit })}
          >
            <option value="day">days</option>
            <option value="week">weeks</option>
            <option value="month">months</option>
            <option value="quarter">quarters</option>
            <option value="year">years</option>
          </select>
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <select
            aria-label="specific op"
            className={input}
            value={spec.op}
            onChange={(e) => emit({ ...spec, op: e.target.value as SpecVal["op"] })}
          >
            <option value="on">On</option>
            <option value="before">Before</option>
            <option value="after">After</option>
            <option value="between">Between</option>
          </select>
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
