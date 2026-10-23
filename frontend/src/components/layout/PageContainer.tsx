import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * Every routed page's outer frame (FE-PAGE-6): one padding, one left edge.
 *
 * - `full` — lists, tables, Control Plane pages and tool pages (SQL Lab, the
 *   chart and dashboard editors, an endpoint's detail): the width `main` gives.
 * - `narrow` — forms, form editors (an alert or report, a create page) and the
 *   Workspace settings pane: a 960px column (`max-w-page`) that starts at the
 *   SAME left edge as a list. Never centred: moving from a list to its editor
 *   must not shift the content sideways (KI-215).
 *
 * `box-content` so `max-w-page` caps the content box and the padding sits
 * outside it: the column is 960px of content on every narrow page, whatever
 * the padding.
 */
export type PageWidth = "full" | "narrow";

export function PageContainer({
  width = "full",
  className,
  children,
}: {
  width?: PageWidth;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      data-page-width={width}
      className={cn("p-4", width === "narrow" && "box-content max-w-page", className)}
    >
      {children}
    </div>
  );
}
