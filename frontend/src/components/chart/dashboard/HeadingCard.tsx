import { useEffect, useRef } from "react";

/** A dashboard heading card: a single styled line, no card chrome. In edit mode it is an inline
 *  input bound to the raw text (so {{slug}} stays literal). */
export function HeadingCard({ text, editing, autoFocus, onChange }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }) {
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
      <input
        ref={ref}
        aria-label="heading text"
        className="dash-nodrag h-full w-full bg-transparent px-1 text-xl font-bold text-foreground placeholder:font-normal placeholder:text-muted-foreground focus:outline-none"
        placeholder="Heading"
        autoFocus={autoFocus}
        value={text}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <div className="flex h-full items-center px-1 text-xl font-bold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
