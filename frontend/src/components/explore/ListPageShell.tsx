import type { ReactNode } from "react";

/**
 * The frame every Explore list page shares: `p-4`, one `h1`, a right-aligned
 * toolbar, and a primary action.
 *
 * The `p-4` + `flex items-center justify-between` header is written out by hand
 * in all six list pages, and three of them carry a comment calling it "the
 * canonical Explore list-page scaffold" — a convention documented six times and
 * enforced nowhere.
 *
 * It takes `toolbar` and `action` as nodes rather than as props describing what
 * they contain. A shell that knew about bookmarks, bulk mode and trash would
 * have to grow a prop per capability, and the pages differ in which they have.
 *
 * The header adds `flex-wrap` and `gap-3` on top of what the six hand-written
 * headers use today. Neither is a restyle: `Endpoints.tsx` puts two buttons in
 * its action slot beside a title, and without wrapping that row overflows on a
 * narrow viewport — this shell fixes a real defect the current pages have
 * rather than reproducing it. Vertical alignment (`items-center` vs
 * `items-start`) stays conditional on `description` below for the same
 * reason: with no description the title is a single line, so `items-center`
 * matches all six pages' current behavior exactly; with a description the
 * heading block is taller than the button row, and only `items-start` looks
 * right.
 */
export function ListPageShell({
  title,
  description,
  toolbar,
  action,
  children,
}: {
  title: string;
  description?: string;
  toolbar?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  const alignClass = description ? "items-start" : "items-center";
  return (
    <div className="p-4">
      <header className={`mb-4 flex flex-wrap ${alignClass} justify-between gap-3`}>
        <div className="min-w-0">
          <h1 className="text-lg font-semibold">{title}</h1>
          {description ? (
            <p className="mt-0.5 text-sm text-muted-foreground">{description}</p>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          {toolbar}
          {action}
        </div>
      </header>
      {children}
    </div>
  );
}
