import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import type { ReactNode } from "react";

export function StudioToolbar({
  name, onName, description, onDescription, autoRun, onAutoRun, onRun, onSave, running, saving, saveLabel,
  exportMenu, sqlDisclosure,
}: {
  name: string; onName: (v: string) => void;
  /** Edited here, beside the name, so saving an existing chart needs no dialog (KI-008). */
  description: string; onDescription: (v: string) => void;
  autoRun: boolean; onAutoRun: (v: boolean) => void;
  onRun: () => void; onSave: () => void; running: boolean; saving?: boolean; saveLabel: string;
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
          <Button size="sm" variant="outline" onClick={onSave} disabled={saving}>{saving ? "Saving…" : saveLabel}</Button>
          {exportMenu}
        </div>
      </div>
      <input aria-label="Chart description" value={description} onChange={(e) => onDescription(e.target.value)}
        placeholder="Add a description"
        className="-mt-2 h-7 w-full min-w-0 rounded-md border border-transparent bg-transparent px-1.5 text-xs text-muted-foreground outline-none transition-colors placeholder:text-muted-foreground/70 hover:border-input focus:bg-background focus:text-foreground focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring" />
      {sqlDisclosure}
    </>
  );
}
