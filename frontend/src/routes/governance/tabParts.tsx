import { useId, type ReactNode } from "react";
import { Link } from "react-router-dom";
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

/**
 * A table's Edit and Delete column, pinned to the right edge of the table's
 * own scroll container: on a narrow screen the other columns scroll under it
 * and the actions stay in view (D10). The page itself never scrolls sideways.
 */
export const ACTIONS_CELL = "sticky right-0 w-24 bg-background";

/**
 * I2: the grants a Data page lists are egret's (`GET /perm/grants`) only. Access
 * can also come from hand-written Ranger policies and from the engine policies
 * egret seeds itself (the platform-admin set, the per-user baseline, the
 * delegator's impersonate), which no egret grant row describes. Explain
 * evaluates what the engines load, so the note points at it, when the viewer
 * may run it (`policy:read`).
 */
export function EgretGrantsOnlyNote({
  explainLabel,
  explainTo,
  subject = "egret grants",
}: {
  explainLabel: string | null;
  /** When set, `explainLabel` is a link there (a People page's Check access tab). */
  explainTo?: string;
  /** What the list holds: `egret grants` (the default) or `egret rules` (P10). */
  subject?: string;
}) {
  const lead = `Lists ${subject}. Hand-written and egret-seeded engine policies are not listed here`;
  if (!explainLabel) return <p className="text-xs text-muted-foreground">{`${lead}.`}</p>;
  return (
    <p className="text-xs text-muted-foreground">
      {`${lead}; `}
      {explainTo ? (
        <Link to={explainTo} className="text-primary underline">
          {explainLabel}
        </Link>
      ) : (
        explainLabel
      )}
      {" gives the engines' answer."}
    </p>
  );
}
