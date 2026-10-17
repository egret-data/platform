import type { ReactNode } from "react";
import type { GovernanceCatalog } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { CatalogState } from "@/routes/governance/data/objectData";
import { TabError } from "@/routes/governance/tabParts";

/**
 * A table's Edit and Delete column, pinned to the right edge of the table's
 * own scroll container: on a narrow screen the other columns scroll under it
 * and the actions stay in view (D10). The page itself never scrolls sideways.
 */
export const ACTIONS_CELL = "sticky right-0 w-24 bg-background";

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

/**
 * I2: the grants a Data page lists are egret's (`GET /perm/grants`) only. Access
 * can also come from hand-written Ranger policies and from the engine policies
 * egret seeds itself (the platform-admin set, the per-user baseline, the
 * delegator's impersonate), which no egret grant row describes. Explain
 * evaluates what the engines load, so the note points at it, when the viewer
 * may run it (`policy:read`).
 */
export function EgretGrantsOnlyNote({ explainLabel }: { explainLabel: string | null }) {
  const explain = explainLabel ? `; ${explainLabel} gives the engines' answer.` : ".";
  return (
    <p className="text-xs text-muted-foreground">
      {`Lists egret grants. Hand-written and egret-seeded engine policies are not listed here${explain}`}
    </p>
  );
}
