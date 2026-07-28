import type { ReactNode } from "react";

export function SectionLabel({ children }: { children: ReactNode }) {
  return <h4 className="border-b pb-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">{children}</h4>;
}
