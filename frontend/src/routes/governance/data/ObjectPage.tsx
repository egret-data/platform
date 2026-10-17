import { useParams, useSearchParams } from "react-router-dom";
import { dottedPath } from "@/routes/governance/data/CatalogTree";
import { urlToPath } from "@/routes/governance/data/objectPath";

/**
 * An object's page: a catalog, a schema, or a table or view, named by the URL.
 * This frame shows the name and the full path; the header, its actions and the
 * tabs (spec §2.2) fill it in from the next task.
 */
export default function ObjectPage() {
  const [search] = useSearchParams();
  const path = urlToPath(useParams(), search);
  const name = path.object ?? path.schema ?? path.catalog;
  return (
    <div className="space-y-1">
      <h1 className="text-2xl font-semibold break-all">{name}</h1>
      <p className="font-mono text-xs break-all text-muted-foreground">{dottedPath(path)}</p>
    </div>
  );
}
