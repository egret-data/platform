import type { ComponentPropsWithoutRef } from "react";
import { cn } from "@/lib/utils";

/**
 * The page's one `h1`, on one scale (FE-PAGE-7, KI-214).
 *
 * - `page` — 20px (`text-xl`): every list, detail, form and settings page.
 * - `compact` — 16px (`text-base`): the header bar of a full-height tool, where
 *   the work surface needs the height (SQL Lab, the chart editor, an
 *   endpoint's detail, an app's viewer and editor).
 *
 * No other size is a page title. A section inside a page is an `h2` below it.
 */
export type PageTitleSize = "page" | "compact";

export const PAGE_TITLE_CLASS: Record<PageTitleSize, string> = {
  page: "text-xl font-semibold leading-tight",
  compact: "text-base font-semibold leading-tight",
};

export function PageTitle({
  size = "page",
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<"h1"> & { size?: PageTitleSize }) {
  return (
    <h1 data-page-title={size} className={cn(PAGE_TITLE_CLASS[size], className)} {...props}>
      {children}
    </h1>
  );
}
