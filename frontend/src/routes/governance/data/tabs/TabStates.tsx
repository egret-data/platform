import type { ReactNode } from "react";
import type { GovernanceCatalog } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { CatalogState } from "@/routes/governance/data/objectData";
import { TabError } from "@/routes/governance/tabParts";

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
