import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { PEOPLE_BASE } from "@/routes/governance/people/peopleUrl";

/*
 * Parts the single-item pages share (`GrantPage`, `RulePage`; spec §2 "Single
 * items"): the header a principal page has (the way back, the kind, the
 * title, the actions), a missing item's sentence, and one fact of the item.
 */

/** The way back to People & groups, the item's kind above its title, and its actions. */
export function ItemHeader({ kind, title, actions }: { kind: string; title: ReactNode; actions?: ReactNode }) {
  return (
    <header className="space-y-2">
      <Link to={PEOPLE_BASE} className="text-sm text-muted-foreground hover:underline">
        People & groups
      </Link>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs uppercase text-muted-foreground">{kind}</p>
          <h1 className="break-words text-lg font-semibold">{title}</h1>
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </header>
  );
}

/** An id that names nothing: `No grant has this id; it may have been deleted.` */
export function ItemMissing({ text }: { text: string }) {
  return (
    <Alert>
      <AlertDescription>{text}</AlertDescription>
    </Alert>
  );
}

/** One fact of the item, in the page's description list. */
export function Detail({ term, children }: { term: string; children: ReactNode }) {
  return (
    <>
      <dt className="font-medium text-muted-foreground">{term}</dt>
      <dd className="min-w-0 space-y-1 break-words">{children}</dd>
    </>
  );
}
