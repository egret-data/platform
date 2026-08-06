import { useMemo, useRef, useState } from "react";
import { ChevronsUpDown, Check } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { CURRENCIES } from "../currencies";

/**
 * A SEARCHABLE PICKER OVER THE 122 NAMED CURRENCIES, replacing a free-text box.
 *
 * WHY NOT THE `Select` EVERY OTHER CONTROL IN THIS PANEL USES. Radix's `Select` gives a keyboard
 * path and a portal for free and would have been the smaller change, but it has no filter, and a
 * scrolling list of 122 entries is a worse control than the text box it replaces for anyone who
 * knows the code and a barely better one for anyone who does not. Its built-in typeahead does jump
 * on a prefix, but only on a prefix — "dong" finds nothing, and there is no way to see that the
 * search is what failed. Metabase's own widget is a select with `searchProp: "name"`
 * (`visualizations/lib/settings/column.ts`), i.e. a filtered list, so a filtered list is what this
 * is.
 *
 * WHY NOT A NEW DEPENDENCY. `cmdk` (which `shadcn`'s Combobox recipe is built on) is not in this
 * tree and neither is `@radix-ui/react-toggle-group`; adding a package for one control in one panel
 * is not a trade this PR is allowed to make. So it is Popover + an input + a list, which is the same
 * three primitives, in the same order, with the same class vocabulary, as `ui/tag-combobox.tsx` —
 * the one other filtered picker in the app. It is NOT shared with that one: `TagCombobox` is
 * multi-select over an async suggestion endpoint, and the parts that would have to be pulled apart
 * to serve both are exactly the parts that differ.
 *
 * THE KEYBOARD PATH IS THE POINT OF THE `activeIndex` STATE, not decoration. Focus stays in the
 * search input the whole time — moving DOM focus onto each option as it is highlighted would make
 * every arrow key steal the caret and end typing — so the highlight is tracked here and published
 * through `aria-activedescendant`, which is the ARIA pattern for exactly this arrangement. Arrow
 * keys move it, Enter commits it, and Escape falls through to the Popover, which closes.
 *
 * WHAT IS STORED IS THE CODE. `format.ts`'s `formatNumber` hands `fmt.currency` to
 * `Intl.NumberFormat` as `opts.currency` and every chart already saved holds a code there, so the
 * name is a search key and a label and never leaves this component. `currencies.ts` records why
 * that list is vendored rather than derived from `Intl`.
 */
export function CurrencySelect({ value, onChange, id }: {
  /** The stored ISO code. Undefined reads as USD, which is what `formatNumber` falls back to. */
  value: string | undefined;
  onChange: (code: string) => void;
  id?: string;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const code = value ?? "USD";
  // An unknown stored code — a chart saved through the old free-text box, or hand-edited JSON —
  // still has to READ as something. Showing the raw code beats showing an empty trigger, and it is
  // also the only state in which the trigger's text is not one of the list's names.
  const selectedName = CURRENCIES.find((c) => c.code === code)?.name ?? code;

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return CURRENCIES;
    // Name OR code, because both are things a reader arrives with — Metabase searches the name
    // alone, but its list is reached from a page that already showed the code.
    return CURRENCIES.filter(
      (c) => c.name.toLowerCase().includes(q) || c.code.toLowerCase().includes(q),
    );
  }, [query]);

  const commit = (picked: string) => {
    onChange(picked);
    setOpen(false);
    setQuery("");
  };

  const move = (delta: number) => {
    if (matches.length === 0) return;
    const next = Math.min(matches.length - 1, Math.max(0, activeIndex + delta));
    setActiveIndex(next);
    // The list scrolls; an arrow key that highlights a row nobody can see is not a keyboard path.
    listRef.current?.querySelector(`#currency-opt-${next}`)?.scrollIntoView({ block: "nearest" });
  };

  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        // Reopening should not resume someone else's half-typed search, and the highlight has to
        // start on a row that exists.
        if (o) { setQuery(""); setActiveIndex(0); }
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          id={id}
          role="combobox"
          aria-expanded={open}
          aria-label="Currency"
          // The focus ring is `SelectTrigger`'s, copied class for class, and NOT shadcn's default
          // `focus:ring-2 ring-offset-2`. `src/__tests__/focusIndicatorContrast.test.ts` rejects
          // both halves of that default — a ring OFFSET puts a band of `--background` between the
          // control and its ring, and a bare `focus:` variant draws the ring on the mouse path too
          // — and it caught this component's first draft doing exactly that. One indicator, one
          // shape, product-wide (#419).
          className="flex h-8 w-full items-center justify-between gap-1 rounded-md border border-input bg-transparent px-2 text-left text-sm outline-none transition-[color,box-shadow] focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        >
          <span className="truncate">{selectedName}</span>
          <ChevronsUpDown className="h-4 w-4 shrink-0 text-muted-foreground opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[--radix-popover-trigger-width] min-w-56 p-1">
        <input
          type="text"
          autoFocus
          aria-label="Search currencies"
          aria-controls="currency-listbox"
          aria-activedescendant={matches.length ? `currency-opt-${activeIndex}` : undefined}
          className="mb-1 h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm outline-none placeholder:text-muted-foreground"
          placeholder="Search…"
          value={query}
          onChange={(e) => { setQuery(e.target.value); setActiveIndex(0); }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
            else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
            else if (e.key === "Enter") {
              e.preventDefault();
              if (matches[activeIndex]) commit(matches[activeIndex].code);
            }
          }}
        />
        <div
          ref={listRef}
          id="currency-listbox"
          role="listbox"
          aria-label="Currency"
          className="max-h-56 overflow-y-auto"
        >
          {matches.map((c, i) => (
            <button
              key={c.code}
              id={`currency-opt-${i}`}
              type="button"
              role="option"
              aria-selected={c.code === code}
              onMouseEnter={() => setActiveIndex(i)}
              onClick={() => commit(c.code)}
              className={`flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm ${i === activeIndex ? "bg-accent" : ""}`}
            >
              <span className="truncate">{c.name}</span>
              <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
                {c.code}
                {c.code === code && <Check className="h-3 w-3" />}
              </span>
            </button>
          ))}
          {matches.length === 0 && (
            <div className="px-2 py-1.5 text-sm text-muted-foreground">No currency matches.</div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
