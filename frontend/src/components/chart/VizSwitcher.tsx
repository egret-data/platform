import { useState } from "react";
import { CHART_TYPES } from "./chartTypes";
import { ChartTypeGallery } from "./ChartTypeGallery";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { VizType } from "@/api/charts";

/**
 * The chart-type chip and the gallery behind it. Owns the dialog's open state, since
 * nothing outside needs it — the shell only cares which type came back.
 */
export function VizSwitcher({ viz, onChange }: { viz: VizType; onChange: (v: VizType) => void }) {
  const [open, setOpen] = useState(false);
  const meta = CHART_TYPES.find((t) => t.key === viz);
  const Icon = meta?.icon;
  return (
    <>
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          {Icon && <Icon className="h-4 w-4 text-primary" />}
          <span className="text-sm font-medium">{meta?.name ?? viz}</span>
        </div>
        <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => setOpen(true)}>
          Change
        </Button>
      </div>
      <Dialog open={open} onOpenChange={setOpen}>
        {/* THE HEIGHT CAP MOVED TO THE PRIMITIVE. `DialogContent` used to have no maximum height
            at all, so it grew to its content and centred the OVERFLOW with it — measured at
            1440×900, 1246px tall at `top: -173px`, with the title, the close button and the first
            row of chart types off-screen and unreachable by mouse or keyboard. #441 capped this one
            dialog; the cap now lives in `dialog.tsx`, which is why the `max-h`/`flex-col` that used
            to be spelled out here is gone. Nothing about this dialog's geometry changed.

            What is still local is WHERE the scroll goes. The primitive scrolls the whole body,
            which is right for a form; here it would scroll the search field away with the cards.
            `bodyClassName="flex flex-col"` makes the body a column of definite height instead, so
            the header and the search field keep theirs and the grid inside the gallery — which
            already carries `min-h-0 flex-1 overflow-y-auto` — is what actually scrolls. */}
        <DialogContent bodyClassName="flex flex-col">
          <DialogHeader className="shrink-0"><DialogTitle>Change visualization</DialogTitle></DialogHeader>
          <ChartTypeGallery selected={viz} onSelect={(k) => { onChange(k); setOpen(false); }} />
        </DialogContent>
      </Dialog>
    </>
  );
}
