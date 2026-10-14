import { useCallback, type CSSProperties } from "react";
import { useReactFlow, useStoreApi } from "@xyflow/react";

/**
 * THE CANVAS PIECES W7c's `TopologyView` and W7d's namespace dependencies graph share: the xyflow
 * theme, the zoom range and fit, the roving focus's arrow keys, and keeping the focused node on
 * screen. Moved here from `TopologyView.tsx` unchanged (W7d Task 8); `TopologyView` uses them as
 * before. Like the rest of `topology/`, this module brings `@xyflow/react` and is reached through
 * `lazy()` only.
 */

/**
 * xyflow's stylesheet is unlayered, so it beats Tailwind's utilities; its own variables are the way
 * in. Its attribution's background defaults to a light wash, which shows as a pale box on dark.
 */
export const XY_THEME = { "--xy-attribution-background-color": "var(--background)" } as CSSProperties;

/** Fit: room around the graph, and never blown up past life size. */
export const FIT = { padding: 0.12, maxZoom: 1 } as const;
/** The canvas's zoom range; Fit may go as low as `MIN_ZOOM` on a large graph. */
export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 2;

/** A box on the canvas, in the graph's own pixels, top-left corner. */
export interface CanvasBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Where a roving focus moves on `key`, from `index` in `items` (the layout's order): Right and Down
 * to the next, Left and Up to the previous, Home and End to the ends. `undefined` when there is no
 * such item (the key is still the canvas's); `null` when the key is not one of these (Enter and
 * Space reach the node's own click).
 */
export function rovingTarget<T>(key: string, items: readonly T[], index: number): T | undefined | null {
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
      return items[index + 1];
    case "ArrowLeft":
    case "ArrowUp":
      return items[index - 1];
    case "Home":
      return items[0];
    case "End":
      return items[items.length - 1];
    default:
      return null;
  }
}

/** Keeps a node on screen: centres it, at the current zoom, when it is outside the visible canvas. */
export function useKeepInView(): (box: CanvasBox) => void {
  const rf = useReactFlow();
  const store = useStoreApi();
  return useCallback(
    (box: CanvasBox) => {
      const { width, height } = store.getState();
      const { x, y, zoom } = rf.getViewport();
      const left = box.x * zoom + x;
      const top = box.y * zoom + y;
      if (width > 0 && (left < 0 || top < 0 || left + box.width * zoom > width || top + box.height * zoom > height)) {
        void rf.setCenter(box.x + box.width / 2, box.y + box.height / 2, { zoom });
      }
    },
    [rf, store],
  );
}
