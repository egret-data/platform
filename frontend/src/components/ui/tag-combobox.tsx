import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronsUpDown, Loader2, X } from "lucide-react";
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
  /** id(s) of the element(s) that describe the input (a hint, an error). */
  ariaDescribedBy?: string;
  /**
   * How to render a chip when the VALUE is not what a person should read —
   * an owner is stored by user id, and a row of UUIDs is not a list of people.
   * Defaults to the value itself.
   */
  labelFor?: (value: string) => string;
}

export function TagCombobox({
  values,
  onChange,
  suggest,
  queryKey,
  placeholder,
  creatable = true,
  ariaLabel,
  ariaDescribedBy,
  labelFor,
}: TagComboboxProps) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

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
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {/* The whole box (chips + input) is the popover trigger, so the
            suggestion list anchors below it rather than below a lone input. */}
        <div
          role="button"
          tabIndex={0}
          className="flex min-h-9 w-full flex-wrap items-center gap-1 rounded-md border border-input bg-transparent px-2 py-1 text-sm focus-within:outline-none focus-within:ring-2 focus-within:ring-ring cursor-text"
          onClick={() => {
            setOpen(true);
            inputRef.current?.focus();
          }}
        >
          {values.map((v) => (
            <span
              key={v}
              className="inline-flex items-center gap-1 rounded bg-secondary px-1.5 py-0.5 text-xs text-secondary-foreground"
            >
              {labelFor ? labelFor(v) : v}
              <button
                type="button"
                aria-label={`remove ${labelFor ? labelFor(v) : v}`}
                onClick={(e) => {
                  e.stopPropagation();
                  remove(v);
                }}
              >
                <X className="h-3 w-3" />
              </button>
            </span>
          ))}
          <input
            ref={inputRef}
            // type="text" must be explicit: Radix PopoverTrigger's own `type="button"`
            // wins the Slot prop-merge unless the child sets `type` itself, which would
            // silently turn this into a non-editable input[type=button]. The trigger's
            // Slot child here is the wrapping <div>, which has no `type` attribute, so
            // this input is unaffected — kept explicit anyway in case of refactors that
            // move the trigger prop-merge target back onto the input.
            type="text"
            aria-label={ariaLabel}
            aria-describedby={ariaDescribedBy}
            className="min-w-[80px] flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
            placeholder={placeholder}
            value={q}
            onFocus={() => setOpen(true)}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && creatable) {
                e.preventDefault();
                add(q);
              } else if (e.key === "Backspace" && q === "" && values.length > 0) {
                remove(values[values.length - 1]);
              }
            }}
          />
          {/* Affordance that this is a combobox with suggestions: a spinner while
              the lookup is in flight, otherwise a chevron. */}
          {open && suggestions.isFetching ? (
            <Loader2 className="h-4 w-4 shrink-0 animate-spin text-muted-foreground" />
          ) : (
            <ChevronsUpDown className="h-4 w-4 shrink-0 text-muted-foreground opacity-50" />
          )}
        </div>
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
            {suggestions.isFetching
              ? "Searching…"
              : creatable
                ? "No matches — type a value and press Enter."
                : "No matches."}
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}
