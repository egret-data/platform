import { type ReactNode } from "react";

import { Card } from "@/components/ui/card";

/**
 * A labelled card wrapping one detail section: an uppercase header strip plus a
 * scrollable body. Shared by the catalog table-detail view and the warehouse
 * Statistics / Tasks tabs so all detail surfaces read consistently.
 */
export function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <Card className="overflow-hidden">
      <div className="border-b bg-muted/40 px-4 py-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </div>
      <div className="overflow-x-auto p-4">{children}</div>
    </Card>
  );
}
