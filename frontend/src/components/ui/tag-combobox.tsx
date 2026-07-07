import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

export interface TagComboboxSuggestion {
  value: string;
  label?: string;
  hint?: string;
}

export interface TagComboboxProps {
  values: string[];
  onChange: (values: string[]) => void;
  /** Async suggestions for the current query. */
  suggest: (q: string) => Promise<TagComboboxSuggestion[]>;
  /** react-query key prefix (the query text is appended internally). */
  queryKey: unknown[];
  placeholder?: string;
  /** Allow free-typing a value not in the suggestions (Enter to add). Default true. */
  creatable?: boolean;
  /** aria-label on the input. */
  ariaLabel?: string;
}

export function TagCombobox({
  values,
  onChange,
  suggest,
  queryKey,
  placeholder,
  creatable = true,
  ariaLabel,
}: TagComboboxProps) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);

  const suggestions = useQuery<TagComboboxSuggestion[]>({
    queryKey: [...queryKey, q],
    queryFn: () => suggest(q),
    enabled: open,
    retry: false,
  });

  const filtered = (suggestions.data ?? []).filter((s) => !values.includes(s.value));

  const add = (v: string) => {
    const t = v.trim();
    if (t && !values.includes(t)) onChange([...values, t]);
    setQ("");
  };

  const remove = (v: string) => onChange(values.filter((x) => x !== v));

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1">
        {values.map((v) => (
          <Badge key={v} variant="secondary" className="gap-1">
            {v}
            <Button
              type="button"
              variant="ghost"
              size="iconSm"
              aria-label={`remove ${v}`}
              onClick={() => remove(v)}
            >
              <X className="h-3 w-3" />
            </Button>
          </Badge>
        ))}
      </div>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          {/* type="text" must be explicit: Radix PopoverTrigger's own `type="button"` wins the
              Slot prop-merge unless the child sets `type` itself, which would silently turn
              this into a non-editable input[type=button]. */}
          <Input
            type="text"
            aria-label={ariaLabel}
            placeholder={placeholder}
            value={q}
            onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && creatable) {
                e.preventDefault();
                add(q);
              }
            }}
          />
        </PopoverTrigger>
        <PopoverContent
          align="start"
          className="max-h-64 w-64 overflow-y-auto p-1"
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          {filtered.map((s) => (
            <button
              key={s.value}
              type="button"
              onClick={() => add(s.value)}
              className="flex w-full items-center justify-between rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
            >
              <span>{s.label ?? s.value}</span>
              {s.hint && <span className="text-xs text-muted-foreground">{s.hint}</span>}
            </button>
          ))}
          {filtered.length === 0 && (
            <div className="px-2 py-1.5 text-sm text-muted-foreground">
              {creatable ? "Type and press Enter." : "No matches."}
            </div>
          )}
        </PopoverContent>
      </Popover>
    </div>
  );
}
