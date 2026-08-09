import { useEffect, useState } from "react";

/** Viewport width below which the navigation sidebar must not consume layout width.
 *  Deliberately NOT the grid's 753px breakpoint: that one measures the dashboard CONTAINER,
 *  this one measures the window. Collapsing them into one number would make the sidebar's
 *  behaviour depend on which route is mounted. */
export const NARROW_VIEWPORT_PX = 768;

const QUERY = `(max-width: ${NARROW_VIEWPORT_PX - 1}px)`;

export function useIsNarrowViewport(): boolean {
  const [narrow, setNarrow] = useState(() =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(QUERY).matches
      : false,
  );
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(QUERY);
    const onChange = (e: MediaQueryListEvent) => setNarrow(e.matches);
    setNarrow(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return narrow;
}
