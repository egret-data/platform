import { useEffect, useRef, type ReactNode, type Ref } from "react";

/** The filter list a heading card draws beside its text, plus the two refs the collision test
 *  measures. Supplied by `DashboardCardView`; absent on a heading that hosts nothing, in which
 *  case this component renders exactly what it always did. */
export interface HeadingInlineFilters {
  node: ReactNode;
  containerRef: Ref<HTMLDivElement>;
  textRef: Ref<HTMLDivElement>;
}

/** A dashboard heading card: a single styled line, no card chrome. In edit mode it is an inline
 *  input bound to the raw text (so {{slug}} stays literal). A heading may host dashboard filters
 *  (Metabase's `supportsInlineParameters`), which sit at the trailing edge of the same row —
 *  Heading.tsx:167-215 does exactly this, in both modes. */
export function HeadingCard({ text, editing, autoFocus, onChange, inlineFilters }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void; inlineFilters?: HeadingInlineFilters }) {
  const ref = useRef<HTMLInputElement>(null);
  // Deferred focus: when added via the Radix dropdown, Radix's close teardown blurs focus to
  // <body> after mount, so a plain autoFocus is lost. Re-focus on the next tick.
  useEffect(() => {
    if (!autoFocus) return;
    const id = window.setTimeout(() => ref.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [autoFocus]);
  if (editing && onChange) {
    return (
      <div ref={inlineFilters?.containerRef} className="flex h-full items-center gap-2">
        <div ref={inlineFilters?.textRef} className="min-w-0 flex-1">
          <input
            ref={ref}
            aria-label="heading text"
            className="dash-nodrag h-full w-full bg-transparent px-1 text-xl font-bold text-foreground placeholder:font-normal placeholder:text-muted-foreground focus:outline-none"
            placeholder="Heading"
            autoFocus={autoFocus}
            value={text}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
        {inlineFilters?.node}
      </div>
    );
  }
  return (
    <div ref={inlineFilters?.containerRef} className="flex h-full items-center gap-2 px-1 text-xl font-bold text-foreground">
      <div ref={inlineFilters?.textRef} className="min-w-0 flex-1 truncate">
        {text || <span className="text-muted-foreground">Heading</span>}
      </div>
      {inlineFilters?.node}
    </div>
  );
}
