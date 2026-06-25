import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";

/** Header strip shown in bulk mode: a Bulk/Cancel toggle and (when items are selected) actions. */
export function BulkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return (
    <Button variant={active ? "secondary" : "outline"} onClick={onToggle}>
      {active ? "Cancel" : "Bulk select"}
    </Button>
  );
}

export function BulkBar({ count, children }: { count: number; children: ReactNode }) {
  if (count === 0) return null;
  return <div className="mb-2 flex items-center gap-2 text-sm">{children}</div>;
}
