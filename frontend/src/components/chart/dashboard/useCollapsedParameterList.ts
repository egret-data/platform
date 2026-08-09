import { useCallback, useEffect, useRef, useState } from "react";
import { shouldCollapseParameterList } from "./inlineParameters";

/**
 * Metabase's `useResponsiveParameterList` (dashboard/hooks/use-responsive-parameter-list.ts:23-35),
 * ported. Three refs, because three widths decide the answer:
 *
 *   containerRef — the row that holds everything;
 *   reservedRef  — whatever shares the row (the card title, or the heading text);
 *   listRef      — an ALWAYS-EXPANDED copy of the filter list, rendered invisibly.
 *
 * TWO THINGS THAT LOOK LIKE DETAILS AND ARE NOT.
 *
 * `reservedRef` is measured with `scrollWidth`, not `getBoundingClientRect().width`. The title sits
 * in a flex row with `min-w-0 truncate`, so its BOX shrinks to whatever space is left — the sum
 * could then never exceed the container and the list would never collapse. Metabase avoids this by
 * measuring the text on a canvas (`measureTextWidth`); `scrollWidth` on a truncating element is the
 * DOM's answer to the same question, "how wide would this text like to be".
 *
 * `listRef` points at a copy that is never collapsed. Measuring the VISIBLE list would read the
 * trigger's width once collapsed, decide there is room, expand, and oscillate. Metabase renders the
 * same invisible copy for the same reason (CollapsibleDashboardParameterList.tsx:87-101).
 *
 * jsdom reports 0 for every width and has no ResizeObserver, so this hook is inert there — exactly
 * as `DashboardGridLayout.tsx` is where it guards its own observer. The arithmetic is tested
 * through `shouldCollapseParameterList` with the numbers fed in directly, and the geometry is
 * measured in the browser.
 */
export function useCollapsedParameterList(bufferPx: number) {
  const containerRef = useRef<HTMLDivElement>(null);
  const reservedRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [collapsed, setCollapsed] = useState(false);

  const measure = useCallback(() => {
    const next = shouldCollapseParameterList({
      containerWidth: containerRef.current?.getBoundingClientRect().width ?? 0,
      reservedWidth: reservedRef.current?.scrollWidth ?? 0,
      listWidth: listRef.current?.getBoundingClientRect().width ?? 0,
      bufferPx,
    });
    // Bail on an unchanged answer. A ResizeObserver fires on every pixel of a drag-resize, and a
    // setState with the same value still schedules a render pass on each one.
    setCollapsed((prev) => (prev === next ? prev : next));
  }, [bufferPx]);

  useEffect(() => {
    measure();
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  return { collapsed, containerRef, reservedRef, listRef };
}
