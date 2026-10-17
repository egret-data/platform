import { useMemo } from "react";
import { Link } from "react-router-dom";
import { ExternalLink } from "lucide-react";

import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import type { Scope } from "@/api/generated/model";
import { pathToUrl, type ObjectPath } from "@/routes/governance/data/objectPath";
import { isPattern } from "./permFormat";

/** Which tab of the Data area a link opens: a grant's, or a rule's. */
export type DataTab = "permissions" | "masks";

export type DataLink = {
  to: string;
  /** The scope sets a pattern, so the link stops at the deepest level above it. */
  stoppedAtPattern: boolean;
};

/**
 * The Data-area page of `scope`: the object path down to the first level set
 * to a pattern, which is not an object (`ice.sales.“*”` opens the schema).
 * `catalogNameById` is the Governance catalog list; a catalog it does not
 * list has no page, so no link (null, never a broken one).
 */
export function scopeToDataLink(
  scope: Scope,
  catalogNameById: ReadonlyMap<string, string>,
  tab: DataTab,
): DataLink | null {
  const catalog = catalogNameById.get(scope.catalog_id);
  if (catalog === undefined) return null;
  const path: ObjectPath = { catalog };
  let stoppedAtPattern = false;
  for (const level of ["schema", "table", "column"] as const) {
    const value = scope[level];
    if (!value) break;
    if (isPattern(value)) {
      stoppedAtPattern = true;
      break;
    }
    if (level === "schema") path.schema = value;
    else if (level === "table") path.object = value;
    else path.column = value;
  }
  return { to: pathToUrl(path, tab), stoppedAtPattern };
}

/** Catalog id → registry name, from the list the Data area itself reads (one shared query). */
export function useCatalogNameById(): ReadonlyMap<string, string> {
  const q = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  return useMemo(
    () => new Map(q.data?.status === 200 ? q.data.data.map((c) => [c.id, c.name] as const) : []),
    [q.data],
  );
}

/** A quiet text link to a page; `title` says more when the link lands higher than the row's scope. */
export function OpenInDataLink({ to, subject, stoppedAtPattern = false, label = "Open in Data" }: { to: string; subject: string; stoppedAtPattern?: boolean; label?: string }) {
  return (
    <Link
      to={to}
      className="inline-flex items-center gap-1 text-xs text-primary hover:underline"
      aria-label={
        stoppedAtPattern
          ? `${label}: ${subject} (opens the nearest level above its pattern)`
          : `${label}: ${subject}`
      }
      title={stoppedAtPattern ? "The scope is a pattern, so this opens the nearest level above it." : undefined}
    >
      <ExternalLink className="h-3 w-3" aria-hidden="true" />
      {label}
    </Link>
  );
}
