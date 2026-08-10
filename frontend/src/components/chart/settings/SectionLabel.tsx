import type { ReactNode } from "react";

export function SectionLabel({ children }: { children: ReactNode }) {
  // Sentence case, not ALL-CAPS. Metabase's settings panel has no shouted section headers, and in
  // a panel this dense `SERIES` / `FORMAT` / `FORMATTING` read as emphasis nobody asked for.
  // The callers pass sentence-cased strings; the styling no longer overrides them.
  return <h4 className="border-b pb-1 text-xs font-medium text-muted-foreground">{children}</h4>;
}
