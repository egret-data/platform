import type { ReactNode } from "react";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";

/**
 * The frame every list page shares: `PageContainer`, one `PageTitle` (`h1`), a
 * right-aligned toolbar, and a primary action.
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
  width = "full",
  title,
  description,
  back,
  toolbar,
  action,
  children,
}: {
  /**
   * `full` for a list (the default). `narrow` for the create pages that reuse
   * this frame (`EndpointCreate`, `ClientCreate`, `ServiceAccountCreate`): a
   * form is a 960px column on the list's left edge (FE-PAGE-6).
   */
  width?: "full" | "narrow";
  title: string;
  description?: string;
  /**
   * A way OUT, rendered above the title.
   *
   * It exists because the three Data API create pages put `← Endpoints` in
   * `toolbar`, which renders into the same right-hand group as `action` — so
   * the escape sat one control away from the commit, and every detail screen
   * puts that link top-left instead (`EndpointDetail.tsx`). A slot rather than
   * a `backTo` string for the same reason `toolbar` and `action` are nodes: the
   * caller owns the link, its label and whether it is a `Link` at all.
   *
   * Omitted renders no element at all, like `description` — the six list pages
   * that pass none are unchanged.
   */
  back?: ReactNode;
  toolbar?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  const alignClass = description ? "items-start" : "items-center";
  return (
    <PageContainer width={width}>
      <header className={`mb-4 flex flex-wrap ${alignClass} justify-between gap-3`}>
        {/* `flex-1` with a 20rem basis: a long description shrinks this block
            instead of filling the row and wrapping the action under it, which
            put a create page's primary button on the left, below the prose
            (KI-183). It still wraps once less than 20rem is left. */}
        <div className="min-w-0 flex-[1_1_20rem]">
          {back}
          <PageTitle>{title}</PageTitle>
          {description ? (
            <p className="mt-0.5 text-sm text-muted-foreground">{description}</p>
          ) : null}
        </div>
        {/* `flex-wrap` here too, not only on the header row: the action slot is
            itself a group (Endpoints puts a bulk toggle, "Download OpenAPI" and
            "+ Endpoint" in it), so wrapping only the outer row still overflows
            once that group alone is wider than a narrow viewport. */}
        <div className="flex flex-wrap items-center gap-2">
          {toolbar}
          {action}
        </div>
      </header>
      {children}
    </PageContainer>
  );
}
