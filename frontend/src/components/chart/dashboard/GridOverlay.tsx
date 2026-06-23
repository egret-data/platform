import { useEffect, useRef, useState } from "react";

/** Faint cell grid shown behind the dashboard in edit mode (Metabase-style). Non-interactive. */
export function GridOverlay({ cols, rowPx }: { cols: number; rowPx: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.clientWidth);
    // ResizeObserver is unavailable in some test/SSR environments; the static width still works.
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const colW = w > 0 ? w / cols : 0;
  return (
    <div ref={ref} aria-hidden className="pointer-events-none absolute inset-0 z-0"
      style={colW ? {
        backgroundImage: "linear-gradient(to right, hsl(var(--border)) 1px, transparent 1px), linear-gradient(to bottom, hsl(var(--border)) 1px, transparent 1px)",
        backgroundSize: `${colW}px ${rowPx}px`,
        opacity: 0.4,
      } : undefined} />
  );
}
