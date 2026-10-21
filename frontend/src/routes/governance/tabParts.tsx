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
 * A table whose rows stack into cards below `sm` (People live check D6): the
 * header row is hidden and each cell is a line of its own, labelled by its
 * column through the cell's `data-label` (the first cell, the row's name,
 * carries none). The facts a column states (applied on engines, effect,
 * reason) are then on screen at 390 px, where an inner scroller hid them; the
 * page still never scrolls sideways. From `sm` up it is an ordinary table.
 * A pinned `ACTIONS_CELL` falls back into the card's flow.
 */
export const STACKED_TABLE = [
  "max-sm:block max-sm:[&_thead]:hidden max-sm:[&_tbody]:block",
  "max-sm:[&_tr]:block max-sm:[&_tr]:py-2",
  "max-sm:[&_td]:static max-sm:[&_td]:block max-sm:[&_td]:w-auto max-sm:[&_td]:py-0.5 max-sm:[&_td]:text-left max-sm:[&_td]:whitespace-normal",
  "max-sm:[&_td]:before:block max-sm:[&_td]:before:text-xs max-sm:[&_td]:before:text-muted-foreground max-sm:[&_td]:before:content-[attr(data-label)]",
].join(" ");

/**
 * #4b (spec §6): the line between a page's egret grants (or rules) and its
 * hand-written policies section, replacing the I2 note now that hand-written
 * policies are listed. egret-seeded policies are counted in that section.
 * `explainLabel` names the control that gives the engines' answer (a link
 * when `explainTo` is set); without it (no `policy:read`) the line stops.
 */
export function HandWrittenPointer({
  explainLabel,
  explainTo,
  subject = "egret grants",
  lead = "To see who can actually read, use",
}: {
  explainLabel: string | null;
  explainTo?: string;
  /** What is listed above: `egret grants` (the default) or `Rules`. */
  subject?: string;
  /** The sentence before the control's name. */
  lead?: string;
}) {
  const first = `${subject} are above; hand-written policies below.`;
  if (!explainLabel) return <p className="text-xs text-muted-foreground">{first}</p>;
  const end = explainLabel.endsWith("?") ? "" : ".";
  return (
    <p className="text-xs text-muted-foreground">
      {`${first} ${lead} `}
      {explainTo ? (
        <Link to={explainTo} className="text-primary underline">
          {explainLabel}
        </Link>
      ) : (
        explainLabel
      )}
      {end}
    </p>
  );
}
