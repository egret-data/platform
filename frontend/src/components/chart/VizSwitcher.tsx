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
        <DialogContent>
          <DialogHeader><DialogTitle>Change visualization</DialogTitle></DialogHeader>
          <div className="py-2">
            <ChartTypeGallery selected={viz} onSelect={(k) => { onChange(k); setOpen(false); }} />
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
