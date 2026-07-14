import { Info } from "lucide-react";
import type { ReactNode } from "react";

/**
 * A quiet contextual note for settings pages — an info icon plus muted text on a
 * neutral surface. Deliberately NOT amber (`--highlight`): that token is reserved
 * for a page's single primary emphasis, so passive orientation notes use a plain
 * muted style to avoid reading like a warning.
 */
export function SettingsNote({ children }: { children: ReactNode }) {
  return (
    <div
      className="flex items-start gap-2 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
      role="note"
    >
      <Info className="mt-px h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      <span>{children}</span>
    </div>
  );
}
