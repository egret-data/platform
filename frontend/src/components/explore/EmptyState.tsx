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
}: {
  icon?: ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: ReactNode;
}) {
  return (
    <div className="mx-auto flex max-w-sm flex-col items-center gap-2 py-4 text-center">
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
