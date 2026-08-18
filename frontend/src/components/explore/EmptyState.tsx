import type { ComponentType, ReactNode } from "react";

/**
 * A friendly "nothing here yet" placeholder for empty lists: an icon, a short
 * title, a one-line explanation, and an optional call-to-action. Meant to be
 * passed as `FilterableTable`'s `emptyText` (which accepts a ReactNode) so a
 * first-run list guides the user instead of showing a bare "No results."
 */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  variant = "plain",
}: {
  icon?: ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: ReactNode;
  /**
   * `plain` is the original centred block, sized for `FilterableTable`'s
   * `emptyText` slot. `dashed` is the framed first-run block the six list pages
   * hand-roll today; it is a variant rather than a second component because the
   * content is identical and only the frame differs.
   */
  variant?: "plain" | "dashed";
}) {
  const frame = variant === "dashed" ? "rounded border border-dashed p-8" : "max-w-sm py-4";
  return (
    <div className={`mx-auto flex flex-col items-center gap-2 text-center ${frame}`}>
      {Icon ? (
        <span className="grid h-10 w-10 place-items-center rounded-full bg-muted text-muted-foreground">
          <Icon className="h-5 w-5" />
        </span>
      ) : null}
      <p className="text-sm font-medium text-foreground">{title}</p>
      {description ? (
        <p className="text-sm text-muted-foreground">{description}</p>
      ) : null}
      {action ? <div className="mt-1">{action}</div> : null}
    </div>
  );
}
