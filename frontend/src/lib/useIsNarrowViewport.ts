import { useEffect, useState } from "react";

/** Viewport width below which the navigation sidebar must not consume layout width.
 *  Deliberately NOT the grid's 753px breakpoint: that one measures the dashboard CONTAINER,
 *  this one measures the window. Collapsing them into one number would make the sidebar's
 *  behaviour depend on which route is mounted. */
export const NARROW_VIEWPORT_PX = 768;

/** Window width below which the app sidebar is an icon rail, whatever the stored
 *  preference (KI-234): at 768px the 224px sidebar left a table 480px. Tailwind's
 *  `lg`, so the Workspace settings nav changes form at the same width. */
export const SIDEBAR_RAIL_BELOW_PX = 1024;

/** Whether the window is narrower than `px` (default `NARROW_VIEWPORT_PX`). */
export function useIsNarrowViewport(px: number = NARROW_VIEWPORT_PX): boolean {
  const query = `(max-width: ${px - 1}px)`;
  const [narrow, setNarrow] = useState(() =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(query).matches
      : false,
  );
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(query);
    const onChange = (e: MediaQueryListEvent) => setNarrow(e.matches);
    setNarrow(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [query]);
  return narrow;
}
