import { useId, type ReactNode } from "react";
import { RotateCw } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

/*
 * Parts the Governance areas' tabs share (Data, People): a read's error with a
 * retry, and a titled section. Kept outside both lazy areas so neither imports
 * the other's modules.
 */

/** `The <what> could not be loaded. Retry, or reload the page.` */
export function failedText(what: string): string {
  return `The ${what} could not be loaded. Retry, or reload the page.`;
}

/** A tab's (or a section's) error: what failed, and a retry. Never an empty state. */
export function TabError({ what, onRetry, detail }: { what: string; onRetry: () => void; detail?: string }) {
  return (
    <Alert variant="destructive">
      <AlertDescription className="flex flex-wrap items-center justify-between gap-2">
        <span>{failedText(what)}{detail ? ` ${detail}` : ""}</span>
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RotateCw aria-hidden />
          Retry
        </Button>
      </AlertDescription>
    </Alert>
  );
}

/** A section of a tab: a heading and its content, named by the heading (a `region`). */
export function Section({ title, children }: { title: string; children: ReactNode }) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="space-y-2">
      <h2 id={headingId} className="text-base font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}
