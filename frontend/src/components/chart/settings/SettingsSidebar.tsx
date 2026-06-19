import { useState } from "react";
import type { ColumnFormat, Rule } from "../format-types";
import { FormatSection } from "./FormatSection";
import { ConditionalFormattingSection } from "./ConditionalFormattingSection";

interface ResultColumn { name: string; type: string }

export function SettingsSidebar({
  columns, isPivot, columnSettings, conditionalFormatting, onColumnSettings, onConditional, onClose,
}: {
  columns: ResultColumn[];
  isPivot: boolean;
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];
  onColumnSettings: (v: Record<string, ColumnFormat>) => void;
  onConditional: (r: Rule[]) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<"format" | "conditional">("format");
  return (
    <div className="w-72 shrink-0 border-l pl-3">
      <div className="mb-2 flex items-center justify-between">
        <div className="flex gap-1">
          <button type="button" className={`rounded px-2 py-1 text-xs ${tab === "format" ? "bg-muted font-medium" : ""}`} onClick={() => setTab("format")}>Formatting</button>
          <button type="button" className={`rounded px-2 py-1 text-xs ${tab === "conditional" ? "bg-muted font-medium" : ""}`} onClick={() => setTab("conditional")}>Conditional</button>
        </div>
        <button type="button" aria-label="close settings" className="text-muted-foreground" onClick={onClose}>✕</button>
      </div>
      {tab === "format"
        ? <FormatSection columns={columns} value={columnSettings} onChange={onColumnSettings} />
        : <ConditionalFormattingSection columns={columns} rules={conditionalFormatting} isPivot={isPivot} onChange={onConditional} />}
    </div>
  );
}
