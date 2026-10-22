import { Link } from "react-router-dom";
import { Card } from "@/components/ui/card";

const HOME = { to: "/", label: "Go to the home page" };

/**
 * The 403 page. It says the reader has no access, why when the guard knows
 * (`reason`), and offers a way out (`back`, home by default) so a refused page
 * is never a dead end (classification UX C13.5, N23). Every prop is optional,
 * so a caller that passes none renders the same card with the home link.
 *
 * "403 — Forbidden" stays as the eyebrow: it is the status code a reader may
 * quote to an admin, and routing tests key on it.
 */
export function Forbidden({ reason, back = HOME }: { reason?: string; back?: { to: string; label: string } }) {
  return (
    <div className="p-4 sm:p-10">
      <Card className="space-y-2 p-6">
        <p className="text-xs font-medium text-muted-foreground">403 — Forbidden</p>
        <h1 className="text-lg font-semibold">You don't have access to this page.</h1>
        {reason ? <p className="text-sm text-muted-foreground">{reason}</p> : null}
        <Link to={back.to} className="inline-block text-sm font-medium text-primary hover:underline">
          {back.label}
        </Link>
      </Card>
    </div>
  );
}
