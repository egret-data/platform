import type { LucideIcon } from "lucide-react";
import { Link } from "react-router-dom";

/**
 * One labelled fact in a detail page's header strip.
 *
 * Lives here rather than beside its first user because the shape is the house
 * idiom for "what is this object" — a small caps label with the value in
 * foreground weight, several of them on one wrapping row under the title.
 * `WarehouseDetail` established it; the alerts editor adopts it, and copying
 * it a second time would let the two drift.
 *
 * `to` makes the value a link where the fact names another object worth
 * opening. Without it the value is plain text — a fact that goes nowhere
 * should not look clickable.
 */
export function DetailFact({
  icon: Icon,
  label,
  value,
  to,
  title,
}: {
  icon: LucideIcon;
  label: string;
  value: string;
  to?: string;
  /** Tooltip for a value that is a readable rendering of something exact —
   *  a humanised crontab over its own expression, for instance. */
  title?: string;
}) {
  return (
    <div className="flex items-center gap-1.5 text-muted-foreground">
      <Icon className="h-4 w-4" />
      <span className="text-xs uppercase tracking-wide">{label}</span>
      {to ? (
        <Link to={to} className="text-foreground hover:text-primary hover:underline">
          {value}
        </Link>
      ) : (
        <span className="text-foreground" title={title}>
          {value}
        </span>
      )}
    </div>
  );
}
