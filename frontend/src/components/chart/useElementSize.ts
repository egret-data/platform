import { useEffect, useState, type RefObject } from "react";

/**
 * An element's rendered pixel size, kept current by a ResizeObserver — `undefined` until it has
 * actually been measured.
 *
 * Distinct from the measurement EChartsRenderer does for itself: that one measures the CHART, this
 * one measures a host that contains the chart AND something laid out beside it (the pie's legend),
 * so the layout decision between them can be made from the whole box. Rounded to whole pixels and
 * compared before storing, so sub-pixel layout jitter doesn't re-render every frame.
 */
export function useElementSize<T extends HTMLElement>(ref: RefObject<T | null>) {
  const [size, setSize] = useState<{ width: number; height: number } | undefined>(undefined);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const store = (width: number, height: number) =>
      setSize((prev) => (prev && prev.width === width && prev.height === height ? prev : { width, height }));

    const rect = el.getBoundingClientRect();
    store(Math.round(rect.width), Math.round(rect.height));

    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect ?? el.getBoundingClientRect();
      store(Math.round(r.width), Math.round(r.height));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);

  return size;
}
