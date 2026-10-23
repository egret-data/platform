import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * Which ends of a horizontally scrolling element hide content: `start` when it
 * is scrolled away from its left edge, `end` when more lies to the right.
 * Re-measured on scroll and whenever the element or its content resizes.
 */
export function useHorizontalOverflow<T extends HTMLElement>(): {
  /** Pass as the scrolling element's `ref`. */
  setScroller: (el: T | null) => void;
  start: boolean;
  end: boolean;
} {
  const [el, setEl] = React.useState<T | null>(null);
  const [edges, setEdges] = React.useState({ start: false, end: false });

  React.useEffect(() => {
    if (!el) return;
    const measure = () => {
      // 1px of slack: fractional widths leave `scrollWidth` a pixel over.
      const start = el.scrollLeft > 1;
      const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setEdges((prev) => (prev.start === start && prev.end === end ? prev : { start, end }));
    };
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    if (el.firstElementChild) ro?.observe(el.firstElementChild);
    return () => {
      el.removeEventListener("scroll", measure);
      ro?.disconnect();
    };
  }, [el]);

  return { setScroller: setEl, ...edges };
}

/**
 * The cue that a strip scrolls sideways (KI-233): a soft shade on each edge
 * that has content past it. Place it inside a `relative` wrapper AROUND the
 * scrolling element (inside it, the shade would scroll away with the
 * content). A tint of the text colour, so it reads on a page, a card or a
 * muted panel alike, in light and dark.
 */
export function ScrollCue({ start, end }: { start: boolean; end: boolean }) {
  const shade = "pointer-events-none absolute inset-y-0 w-6 from-foreground/10 to-transparent";
  return (
    <>
      {start ? <div aria-hidden="true" data-scroll-cue="start" className={cn(shade, "left-0 bg-gradient-to-r")} /> : null}
      {end ? <div aria-hidden="true" data-scroll-cue="end" className={cn(shade, "right-0 bg-gradient-to-l")} /> : null}
    </>
  );
}

/**
 * A horizontally scrolling strip with the edge cue: for an underline tab strip
 * or a router-link section nav that can be wider than the screen. `className`
 * goes on the scrolling element (pass `tabsListClass("underline")` there).
 */
export function ScrollStrip({
  className,
  children,
  ...props
}: React.ComponentProps<"nav">) {
  const { setScroller, start, end } = useHorizontalOverflow<HTMLElement>();
  return (
    <div className="relative">
      <nav ref={setScroller} className={className} {...props}>
        {children}
      </nav>
      <ScrollCue start={start} end={end} />
    </div>
  );
}

/**
 * A block that scrolls sideways, with the edge cue: the wrapper for a
 * hand-written list `<table>` (the Explore lists), which `ui/table` does not
 * reach. At 390px the dashboards list's Updated column sat past the edge with
 * nothing to show it was there (KI-233).
 */
export function ScrollX({ className, children }: { className?: string; children: React.ReactNode }) {
  const { setScroller, start, end } = useHorizontalOverflow<HTMLDivElement>();
  return (
    <div className="relative">
      <div ref={setScroller} data-scroll-x className={cn("overflow-x-auto", className)}>
        {children}
      </div>
      <ScrollCue start={start} end={end} />
    </div>
  );
}
