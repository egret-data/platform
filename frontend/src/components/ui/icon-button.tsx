import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A square icon-only control with a DECLARED hit target.
 *
 * It exists because the reorder arrows in the chart settings panel were bare `▲`/`▼` glyphs at
 * `text-[10px]` with no box, so their size was whatever the glyph happened to be — measured at
 * 8×10 px, about one sixth the area of Metabase's 22×22 equivalent. They appeared on 10 of the 19
 * chart types, and the table panel alone carries 22 of them, two per column. These are controls
 * people press repeatedly while arranging a chart.
 *
 * `size-6` (24px) is the floor, not the aspiration — pass `size-8` where the row has room.
 *
 * `forwardRef`, not a plain function: a plain-function primitive breaks Radix `asChild`, which
 * this codebase has been bitten by before.
 */
export const IconButton = React.forwardRef<HTMLButtonElement, React.ComponentProps<"button">>(
  ({ className, ...props }, ref) => (
    <button
      ref={ref}
      type="button"
      className={cn(
        "inline-flex size-6 shrink-0 items-center justify-center rounded-md leading-none",
        "text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
        "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
        "disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
      {...props}
    />
  ),
);
IconButton.displayName = "IconButton";
