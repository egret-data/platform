import * as React from "react";
import * as ResizablePrimitive from "react-resizable-panels";

import { cn } from "@/lib/utils";
import { GripVertical } from "lucide-react";

function ResizablePanelGroup({
  className,
  ...props
}: React.ComponentProps<typeof ResizablePrimitive.PanelGroup>) {
  return (
    <ResizablePrimitive.PanelGroup
      data-slot="resizable-panel-group"
      className={cn(
        "flex h-full w-full data-[panel-group-direction=vertical]:flex-col",
        className,
      )}
      {...props}
    />
  );
}

function ResizablePanel({
  ...props
}: React.ComponentProps<typeof ResizablePrimitive.Panel>) {
  return <ResizablePrimitive.Panel data-slot="resizable-panel" {...props} />;
}

function ResizableHandle({
  withHandle,
  className,
  ...props
}: React.ComponentProps<typeof ResizablePrimitive.PanelResizeHandle> & {
  withHandle?: boolean;
}) {
  return (
    <ResizablePrimitive.PanelResizeHandle
      data-slot="resizable-handle"
      className={cn(
        "bg-border relative flex w-px items-center justify-center",
        "after:absolute after:inset-y-0 after:left-1/2 after:w-1 after:-translate-x-1/2",
        // The product's one focus indicator: a solid 2px `--ring`, flush to the
        // box. It used to be `ring-1 + ring-offset-1 + ring-offset-background`,
        // and this handle is where the offset was hardest to defend. The handle
        // is a 1px sliver of `--border` between two panels, so the offset drew a
        // 1px band of `--background` immediately beside a 1px divider — and
        // `--background` is only the right colour when the panels either side are
        // themselves `--background`. In SQL Lab and the notebook they are not
        // (`--card`, and the editor's own surface), so the gap read as the
        // divider flickering rather than as focus. Flush removes the band
        // entirely and doubles the thickness that was carrying the indicator.
        //
        // Both focus utilities are in ONE string on purpose: the ring colour used
        // to sit in the first `cn()` argument and its width in this one, which
        // meant no per-class-string sweep could see them together and the width
        // could be deleted without any assertion noticing.
        "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-hidden",
        "data-[panel-group-direction=vertical]:h-px data-[panel-group-direction=vertical]:w-full data-[panel-group-direction=vertical]:after:left-0 data-[panel-group-direction=vertical]:after:h-1 data-[panel-group-direction=vertical]:after:w-full data-[panel-group-direction=vertical]:after:translate-x-0 data-[panel-group-direction=vertical]:after:-translate-y-1/2 [&[data-panel-group-direction=vertical]>div]:rotate-90",
        "data-[resize-handle-state=hover]:bg-primary data-[resize-handle-state=hover]:ring-1 data-[resize-handle-state=hover]:ring-primary",
        "data-[resize-handle-state=drag]:bg-primary data-[resize-handle-state=drag]:ring-1 data-[resize-handle-state=drag]:ring-primary",
        className,
      )}
      {...props}
    >
      {withHandle && (
        <div className="bg-border z-10 flex h-4 w-3 items-center justify-center rounded-xs border">
          <GripVertical className="size-2.5" />
        </div>
      )}
    </ResizablePrimitive.PanelResizeHandle>
  );
}

export { ResizableHandle, ResizablePanel, ResizablePanelGroup };
