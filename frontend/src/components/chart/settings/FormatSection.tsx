import { useState, useEffect } from "react";
import { Hash, Calendar, Type, type LucideIcon } from "lucide-react";
import { isNumericType, isTemporalType, type ColumnFormat } from "../format-types";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

function colIcon(type: string): LucideIcon {
  if (isNumericType(type)) return Hash;
  if (isTemporalType(type)) return Calendar;
  return Type;
}

function colKind(type: string): string {
  if (isNumericType(type)) return "Number";
  if (isTemporalType(type)) return "Date";
  return "Text";
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function ToggleRow({ id, label, checked, onChange }: {
  id: string; label: string; checked: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function FormatSection({
  columns, value, onChange,
}: {
  columns: ResultColumn[];
  value: Record<string, ColumnFormat>;
  onChange: (v: Record<string, ColumnFormat>) => void;
}) {
  const [sel, setSel] = useState<string | null>(columns[0]?.name ?? null);

  useEffect(() => {
    if (!sel || !columns.find((c) => c.name === sel)) setSel(columns[0]?.name ?? null);
  }, [columns, sel]);

  const col = columns.find((c) => c.name === sel);
  const fmt = (sel && value[sel]) || {};

  function patch(p: Partial<ColumnFormat>) {
    if (!sel) return;
    onChange({ ...value, [sel]: { ...fmt, ...p } });
  }

  return (
    <div className="space-y-2 text-sm">
      {/* column picker — one row per column, with a data-type icon */}
      <div className="flex flex-col gap-0.5">
        {columns.map((c) => {
          const Icon = colIcon(c.type);
          return (
            <button
              key={c.name}
              type="button"
              onClick={() => setSel(c.name)}
              className={`flex items-center gap-2 rounded px-2 py-1.5 text-left ${sel === c.name ? "bg-muted font-medium" : "hover:bg-muted/50"}`}
            >
              <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span className="flex-1 truncate">{c.name}</span>
            </button>
          );
        })}
      </div>

      {/* settings card scoped to the selected column */}
      {col && (
        <div className="space-y-3 rounded-md border bg-muted/20 p-3">
          <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            {(() => { const Icon = colIcon(col.type); return <Icon className="h-3.5 w-3.5" />; })()}
            <span className="truncate text-foreground">{col.name}</span>
            <span className="ml-auto rounded bg-muted px-1.5 py-0.5">{colKind(col.type)}</span>
          </div>

          {isNumericType(col.type) && (
            <div className="space-y-3">
              <Field label="Number style">
                <Select value={fmt.number_style ?? "decimal"}
                  onValueChange={(v) => patch({ number_style: v as ColumnFormat["number_style"] })}>
                  <SelectTrigger aria-label="Number style" className="h-8"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="decimal">Normal</SelectItem>
                    <SelectItem value="percent">Percent</SelectItem>
                    <SelectItem value="currency">Currency</SelectItem>
                    <SelectItem value="scientific">Scientific</SelectItem>
                  </SelectContent>
                </Select>
              </Field>

              {fmt.number_style === "currency" && (
                <div className="flex gap-2">
                  <Field label="Currency">
                    <Input className="h-8" value={fmt.currency ?? "USD"}
                      onChange={(e) => patch({ currency: e.target.value.toUpperCase() })} />
                  </Field>
                  <Field label="Display">
                    <Select value={fmt.currency_style ?? "symbol"}
                      onValueChange={(v) => patch({ currency_style: v as ColumnFormat["currency_style"] })}>
                      <SelectTrigger aria-label="Currency display" className="h-8"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        <SelectItem value="symbol">$</SelectItem>
                        <SelectItem value="code">USD</SelectItem>
                        <SelectItem value="name">dollars</SelectItem>
                        <SelectItem value="narrowSymbol">$ (narrow)</SelectItem>
                      </SelectContent>
                    </Select>
                  </Field>
                </div>
              )}

              <Field label="Decimal places">
                <Input type="number" min={0} className="h-8" placeholder="Auto"
                  value={fmt.decimals ?? ""}
                  onChange={(e) => patch({ decimals: e.target.value === "" ? undefined : Number(e.target.value) })} />
              </Field>

              <div className="flex gap-2">
                <Field label="Prefix">
                  <Input className="h-8" value={fmt.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} />
                </Field>
                <Field label="Suffix">
                  <Input className="h-8" value={fmt.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} />
                </Field>
              </div>

              <div className="space-y-2.5 pt-1">
                <ToggleRow id="fmt-compact" label="Compact (1.2K)" checked={!!fmt.compact} onChange={(v) => patch({ compact: v })} />
                <ToggleRow id="fmt-negatives" label="Negatives in ( )" checked={!!fmt.negative_in_parentheses} onChange={(v) => patch({ negative_in_parentheses: v })} />
              </div>
            </div>
          )}

          {isTemporalType(col.type) && (
            <div className="space-y-3">
              <Field label="Date style">
                <Select value={fmt.date_style ?? "MMMM D, YYYY"} onValueChange={(v) => patch({ date_style: v })}>
                  <SelectTrigger aria-label="Date style" className="h-8"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="MMMM D, YYYY">January 31, 2024</SelectItem>
                    <SelectItem value="M/D/YYYY">1/31/2024</SelectItem>
                    <SelectItem value="D/M/YYYY">31/1/2024</SelectItem>
                    <SelectItem value="YYYY-MM-DD">2024-01-31</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
              <ToggleRow id="fmt-abbreviate" label="Abbreviate" checked={!!fmt.date_abbreviate} onChange={(v) => patch({ date_abbreviate: v })} />
            </div>
          )}

          {!isNumericType(col.type) && !isTemporalType(col.type) && (
            <p className="text-xs text-muted-foreground">Text columns have no format options.</p>
          )}
        </div>
      )}
    </div>
  );
}
