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
        {/* THE DIALOG HAS TO BE TOLD HOW TALL IT MAY BE. `DialogContent` is a centred
            `position: fixed` grid with no maximum height, so it grows to its content and centres
            the OVERFLOW as well — half above the viewport, half below, with nothing to scroll
            because Radix sets `overflow: hidden` on `body` while it is open. Measured at 1440×900
            before this line: 1246px tall at `top: -173px`, so the title, the close button and the
            first row of chart types were off-screen and unreachable by mouse or keyboard.

            `flex flex-col` (overriding the primitive's `grid`) so the header keeps its height and
            the gallery takes what is left; `min-h-0` on the gallery inside is what actually lets it
            shrink and scroll — without it a flex child refuses to go below its content height and
            the overflow comes straight back.

            `dvh`, not `vh`: on mobile Safari `vh` is the LARGEST viewport, which is the one behind
            the browser chrome, so a `vh` cap still overflows by the height of the toolbar. */}
        <DialogContent className="flex max-h-[calc(100dvh-4rem)] flex-col">
          <DialogHeader className="shrink-0"><DialogTitle>Change visualization</DialogTitle></DialogHeader>
          <ChartTypeGallery selected={viz} onSelect={(k) => { onChange(k); setOpen(false); }} />
        </DialogContent>
      </Dialog>
    </>
  );
}
