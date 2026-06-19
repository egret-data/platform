import { useState } from "react";
import type { Rule, SingleColorRule } from "../format-types";
import { RuleEditor } from "./RuleEditor";

interface ResultColumn { name: string; type: string }

function describeRule(rule: Rule): string {
  if (rule.type === "single") return `${rule.columns.join(", ")} ${rule.operator} ${rule.value ?? ""}`;
  return `${rule.columns.join(", ")} color range`;
}

export function ConditionalFormattingSection({
  columns, rules, isPivot, onChange,
}: {
  columns: ResultColumn[];
  rules: Rule[];
  isPivot: boolean;
  onChange: (r: Rule[]) => void;
}) {
  const [editing, setEditing] = useState<number | null>(null);

  if (editing !== null) {
    const rule = rules[editing] ?? ({ columns: [], type: "single", operator: ">", value: "", color: "#ef4444" } as SingleColorRule);
    return (
      <RuleEditor
        columns={columns}
        rule={rule}
        isPivot={isPivot}
        onCancel={() => setEditing(null)}
        onSave={(r) => {
          const next = [...rules];
          if (editing >= rules.length) next.push(r); else next[editing] = r;
          onChange(next);
          setEditing(null);
        }}
      />
    );
  }

  const move = (i: number, dir: -1 | 1) => {
    const j = i + dir;
    if (j < 0 || j >= rules.length) return;
    const next = [...rules];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };

  return (
    <div className="space-y-2 text-sm">
      <button type="button" className="rounded border px-3 py-1" onClick={() => setEditing(rules.length)}>+ Add rule</button>
      {rules.length === 0 && <p className="text-xs text-muted-foreground">No rules yet.</p>}
      {rules.map((rule, i) => (
        <div key={i} className="flex items-center justify-between rounded border px-2 py-1">
          <button type="button" className="flex items-center gap-2 text-left" onClick={() => setEditing(i)}>
            <span className="inline-block h-4 w-4 rounded" style={{ backgroundColor: rule.type === "single" ? rule.color : rule.colors[rule.colors.length - 1] }} />
            <span className="text-xs">{describeRule(rule)}</span>
          </button>
          <div className="flex gap-1">
            <button type="button" aria-label="up" onClick={() => move(i, -1)}>↑</button>
            <button type="button" aria-label="down" onClick={() => move(i, 1)}>↓</button>
            <button type="button" aria-label="delete" className="text-destructive" onClick={() => onChange(rules.filter((_, k) => k !== i))}>✕</button>
          </div>
        </div>
      ))}
    </div>
  );
}
