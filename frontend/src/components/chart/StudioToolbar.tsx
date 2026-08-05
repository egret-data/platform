import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import type { ReactNode } from "react";

export function StudioToolbar({
  name, onName, autoRun, onAutoRun, onRun, onSave, running, saveLabel, exportMenu, sqlDisclosure,
}: {
  name: string; onName: (v: string) => void;
  autoRun: boolean; onAutoRun: (v: boolean) => void;
  onRun: () => void; onSave: () => void; running: boolean; saveLabel: string;
  exportMenu: ReactNode; sqlDisclosure: ReactNode;
}) {
  return (
    <>
      <div className="flex items-center gap-2">
        <input aria-label="Chart name" value={name} onChange={(e) => onName(e.target.value)}
          placeholder="Untitled chart"
          className="h-8 min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-1.5 text-base font-semibold outline-none transition-colors placeholder:font-normal placeholder:text-muted-foreground hover:border-input focus:bg-background focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring" />
        <div className="flex shrink-0 items-center gap-2">
          <Label htmlFor="auto-run" className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Switch id="auto-run" checked={autoRun} onCheckedChange={onAutoRun} aria-label="Auto-run" /> Auto
          </Label>
          <Button size="sm" onClick={onRun} disabled={running}>{running ? "Running…" : "Run"}</Button>
          <Button size="sm" variant="outline" onClick={onSave}>{saveLabel}</Button>
          {exportMenu}
        </div>
      </div>
      {sqlDisclosure}
    </>
  );
}
