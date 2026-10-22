import type { MouseEventHandler, ReactNode } from "react";
import { Link } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { PageTitle, type PageTitleSize } from "./PageTitle";

/**
 * The way back to the list a detail page was opened from (KI-224).
 *
 * One look everywhere: an arrow and the list's name, `text-sm font-medium`, top
 * left above the title. It used to be small grey text with a typed `←`, easy to
 * miss, and on an app's viewer an arrow button inside the title bar instead.
 *
 * `onClick` is for a page that guards leaving with unsaved edits
 * (`EndpointDetail`); it runs before the navigation and may prevent it.
 */
export function BackLink({
  to,
  children,
  onClick,
}: {
  to: string;
  children: ReactNode;
  onClick?: MouseEventHandler<HTMLAnchorElement>;
}) {
  return (
    <Link
      to={to}
      onClick={onClick}
      className="inline-flex items-center gap-1 rounded-sm text-sm font-medium text-muted-foreground hover:text-foreground hover:underline"
    >
      <ArrowLeft aria-hidden="true" className="h-4 w-4 shrink-0" />
      {children}
    </Link>
  );
}

/**
 * Every detail, form and editor page's header, in one shape (FE-PAGE-8,
 * KI-224):
 *
 *   ← Back link
 *   Title  [status]                               [actions]
 *   description
 *   FACT value · FACT value …
 *
 * - `back` — the list this page belongs to. Omitted only where the page has no
 *   parent list (Workspace settings, whose own section nav takes its place).
 * - `status` — a `StatusBadge` (or a link wrapping one) beside the title.
 * - `meta` — a row of `DetailFact`s or chips under the title.
 * - `actions` — the page's buttons, top right.
 *
 * No coloured icon tile: a title is identified by its words and its back link,
 * and only some pages had a tile, which was the inconsistency.
 */
export function DetailHeader({
  back,
  title,
  titleSize = "page",
  titleClassName,
  status,
  description,
  meta,
  actions,
  className,
}: {
  back?: { to: string; label: string; onClick?: MouseEventHandler<HTMLAnchorElement> };
  title: ReactNode;
  titleSize?: PageTitleSize;
  /** Extra classes for the `h1` (`font-mono`, `break-all`, `truncate`…). */
  titleClassName?: string;
  status?: ReactNode;
  description?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <header className={className ?? "mb-4"}>
      {back && (
        <div className="mb-1">
          <BackLink to={back.to} onClick={back.onClick}>
            {back.label}
          </BackLink>
        </div>
      )}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
            <PageTitle size={titleSize} className={titleClassName ?? "min-w-0 break-words"}>
              {title}
            </PageTitle>
            {status}
          </div>
          {description ? <div className="mt-0.5 text-sm text-muted-foreground">{description}</div> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
      {meta ? <div className="mt-2 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">{meta}</div> : null}
    </header>
  );
}
