import { dottedPath, sourceLabel } from "@/routes/governance/data/CatalogTree";
import type { CatalogState } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate } from "@/routes/governance/data/tabs/TabStates";

/**
 * An object's Details tab (spec §2.2): its full path, and its catalog's
 * source, warehouse, workspaces and engines, all from the Governance catalog
 * list (readable under `policy:read` or `tag:read`).
 */
export function DetailsTab({ path, catalog }: { path: ObjectPath; catalog: CatalogState }) {
  return (
    <CatalogGate catalog={catalog}>
      {(c) => {
        const workspaces = c.workspaces.map((w) => w.name).sort((a, b) => a.localeCompare(b));
        const engines = [...c.engines].sort((a, b) => a.localeCompare(b));
        return (
          <dl className="grid grid-cols-1 gap-x-6 gap-y-1 text-sm sm:grid-cols-[10rem_minmax(0,1fr)] sm:gap-y-3">
            <dt className="text-muted-foreground">Full path</dt>
            <dd className="mb-2 font-mono text-xs break-all sm:mb-0">{dottedPath(path)}</dd>
            <dt className="text-muted-foreground">Source</dt>
            <dd className="mb-2 sm:mb-0">{sourceLabel(c)}</dd>
            <dt className="text-muted-foreground">Warehouse</dt>
            <dd className="mb-2 break-all sm:mb-0">{c.warehouseName ?? "None: not an egret catalog"}</dd>
            <dt className="text-muted-foreground">Workspaces</dt>
            <dd className="mb-2 sm:mb-0">{workspaces.length > 0 ? workspaces.join(", ") : "None"}</dd>
            <dt className="text-muted-foreground">Engines</dt>
            <dd>{engines.length > 0 ? engines.join(", ") : "None"}</dd>
          </dl>
        );
      }}
    </CatalogGate>
  );
}
