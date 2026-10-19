import type { Ref } from "@/api/generated/model";
import { pathToUrl } from "@/routes/governance/data/objectPath";
import { PEOPLE_BASE, principalUrl } from "@/routes/governance/people/peopleUrl";

/**
 * Where a finding, a sample or a change's target is fixed (spec §2, "Links
 * per finding kind"). The server sends a typed ref, never a URL, so every
 * link is built here.
 *
 * Data and People names go through `pathToUrl` and `principalUrl`, which keep
 * a `/`, `%`, space or dot-only name in its own segment. A profile name is
 * `encodeURIComponent`d, as every other link to a profile page is
 * (`ProfileDetailPage.tsx`, `OverlapReport.tsx`), because that page reads its
 * param raw. Ids are uuids and need no encoding.
 *
 * `null` for a ref this client does not know: it renders as text.
 */
export function refUrl(r: Ref): string | null {
  switch (r.type) {
    case "grant":
      return `/governance/grants/${r.id}`;
    case "rule":
      return `/governance/rules/${r.id}`;
    case "unmatched":
      return `${PEOPLE_BASE}?kind=unmatched`;
    case "profile":
      return `/governance/policies/${encodeURIComponent(r.name)}`;
    case "profiles":
      return "/governance/policies";
    case "engine":
      return `/control-plane/engines/${r.id}`;
    case "engines":
      return "/control-plane/engines";
    case "catalog":
      return `/control-plane/catalogs/${r.id}`;
    case "catalogs":
      return "/control-plane/catalogs";
    case "object":
      return pathToUrl({ catalog: r.catalog, schema: r.schema, object: r.object }, "lineage");
    case "tag":
      return `/governance/classifications/${r.id}`;
    case "tags":
      return "/governance/classifications";
    case "role":
      return principalUrl({ kind: "role", name: r.name });
    case "data":
      return "/governance/data";
    case "data_catalog":
      return pathToUrl({ catalog: r.name });
    default:
      return null;
  }
}
