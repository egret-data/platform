import type { ReactNode } from "react";
import { RotateCw } from "lucide-react";
import type { GovernanceCatalog } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { CatalogState } from "@/routes/governance/data/objectData";

/**
 * A table's Edit and Delete column, pinned to the right edge of the table's
 * own scroll container: on a narrow screen the other columns scroll under it
 * and the actions stay in view (D10). The page itself never scrolls sideways.
 */
export const ACTIONS_CELL = "sticky right-0 w-24 bg-background";

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

/** Renders `children` once the catalog the URL names is known; its loading, error or absence otherwise. */
export function CatalogGate({
  catalog,
  children,
}: {
  catalog: CatalogState;
  children: (c: GovernanceCatalog) => ReactNode;
}) {
  if (catalog.status === "loading") return <ListSkeleton rows={4} cols={2} />;
  if (catalog.status === "error") return <TabError what="catalog" onRetry={catalog.retry} />;
  if (catalog.status === "missing") {
    return (
      <Alert>
        <AlertDescription>{`No catalog is named “${catalog.name}”.`}</AlertDescription>
      </Alert>
    );
  }
  return <>{children(catalog.catalog)}</>;
}

/** A section of a tab: a heading and its content. */
export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-2">
      <h2 className="text-base font-semibold">{title}</h2>
      {children}
    </section>
  );
}
