import { useState } from "react";
import { Plus, ChevronUp, ChevronDown, Trash2 } from "lucide-react";
import type { Rule, SingleColorRule } from "../format-types";
import { Button } from "@/components/ui/button";
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
    <div className="space-y-1.5 text-sm">
      {rules.length === 0 && <p className="text-xs text-muted-foreground">No rules yet.</p>}
      {rules.map((rule, i) => (
        <div key={i} className="flex items-center gap-1.5 rounded-md border bg-muted/40 px-2 py-1">
          <button type="button" className="flex flex-1 items-center gap-2 overflow-hidden text-left" aria-label={`edit rule ${i + 1}`} onClick={() => setEditing(i)}>
            <span className="inline-block h-4 w-4 shrink-0 rounded border" style={{ backgroundColor: rule.type === "single" ? rule.color : rule.colors[rule.colors.length - 1] }} />
            <span className="truncate text-xs">{describeRule(rule)}</span>
          </button>
          <div className="flex items-center text-muted-foreground">
            <button type="button" aria-label="move rule up" disabled={i === 0} className="rounded p-0.5 hover:bg-accent hover:text-foreground disabled:opacity-30" onClick={() => move(i, -1)}>
              <ChevronUp className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="move rule down" disabled={i === rules.length - 1} className="rounded p-0.5 hover:bg-accent hover:text-foreground disabled:opacity-30" onClick={() => move(i, 1)}>
              <ChevronDown className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="delete rule" className="rounded p-0.5 hover:bg-accent hover:text-destructive" onClick={() => onChange(rules.filter((_, k) => k !== i))}>
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      ))}
      <Button type="button" variant="outline" size="lg" className="w-full" onClick={() => setEditing(rules.length)}>
        <Plus className="mr-1 h-3.5 w-3.5" /> Add rule
      </Button>
    </div>
  );
}
