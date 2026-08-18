import type { ReactNode } from "react";
import { ListChecks } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The bulk-mode toggle, as an icon button in the header toolbar.
 *
 * It used to render a text button reading "Bulk select"/"Cancel". Every page
 * that actually has bulk mode renders an icon button with a tooltip instead, so
 * this file described a shape nothing used — which is consistent with it having
 * had no importers at all.
 */
export function BulkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return <ListToolbarButton icon={ListChecks} label="Bulk select" active={active} onClick={onToggle} />;
}

/**
 * The action strip shown once rows are selected. Renders NOTHING at zero — an
 * always-present empty bar shifts the table down the moment bulk mode is
 * entered, which reads as the page jumping.
 */
export function BulkBar({ count, children }: { count: number; children: ReactNode }) {
  if (count === 0) return null;
  return (
    <div className="mb-2 flex flex-wrap items-center gap-2 text-sm">
      <span className="text-muted-foreground">{count} selected</span>
      {children}
    </div>
  );
}
