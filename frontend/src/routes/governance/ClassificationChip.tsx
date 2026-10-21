import { Link } from "react-router-dom";
import { Lock, X } from "lucide-react";
import { useTags } from "@/api/tags";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Badge } from "@/components/ui/badge";
import { IconButton } from "@/components/ui/icon-button";
import { DEFAULT_COLOR } from "@/routes/governance/classifications/tagParts";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";

/*
 * Kept outside both lazy areas (spec C4): the Data pages and the
 * Classification pages both show chips, and neither may import the other's
 * modules (`scripts/assert-governance-data-lazy.mjs`,
 * `scripts/assert-governance-classification-lazy.mjs`). It imports nothing
 * from `data/`.
 */

/**
 * Whether the viewer may attach and remove classifications on egret's
 * catalogs: `PUT`/`DELETE …/warehouses/{id}/resource-tags` are gated on
 * `catalog_registry:update` (`crates/egret-bi/src/catalog_tags.rs`,
 * `WAREHOUSE_WRITE`); a platform admin passes every key in the PDP.
 */
export function useCanEditCatalogTags(): boolean {
  return usePolicyPermissions()("catalog_registry:update");
}

/** Whether the viewer may open a classification's page (`GET /tags`, `tag:read`; platform admin passes). */
function useCanReadTags(): boolean {
  return usePolicyPermissions()("tag:read");
}

/**
 * One classification on a table or a column (spec C4): a swatch in its colour
 * (user data, as `TagSwatch` draws it, so an inline style; decorative, the
 * name says which), the name as a link to its page (with `tag:read`, else
 * plain text), a lock with the words "requires clearance" when its clearance
 * names anyone (read from the catalogue, `useTags`, by id; never colour
 * alone, FE-A11Y-4), "from table" when a column inherits it, and a remove
 * button naming what it removes from where when `onRemove` is given. An
 * inherited chip never offers Remove: it is the table's, removed there.
 */
export function ClassificationChip({
  tag,
  inherited = false,
  inheritedFrom,
  onRemove,
  objectLabel,
}: {
  tag: { id: string; key: string; name: string; color: string };
  inherited?: boolean;
  /**
   * For an inherited chip, the table it comes from: "from table" links there
   * (spec C4.2). Left out where the chip is already on that table's page.
   */
  inheritedFrom?: { href: string; label: string };
  onRemove?: () => void;
  /** The object it is on, as the remove button names it (`ice.sales.orders.email`). */
  objectLabel: string;
}) {
  const canRead = useCanReadTags();
  // Without tag:read the read is refused: skip it rather than ask again on every chip's mount.
  const tagsQ = useTags({ enabled: canRead });
  const known = tagsQ.data?.find((t) => t.id === tag.id);
  // Read defensively: a stale or partial catalogue row says nothing about clearance rather than crash the chip.
  const gated = !!known && (known.required_roles?.length ?? 0) + (known.required_groups?.length ?? 0) > 0;

  const name = (
    <>
      {tag.name}
      {gated ? (
        <>
          <Lock aria-hidden className="size-3" />
          <span className="sr-only">requires clearance</span>
        </>
      ) : null}
    </>
  );

  return (
    <Badge variant="outline" title={tag.key} className="gap-1.5">
      <span
        aria-hidden="true"
        className="inline-block size-2.5 shrink-0 rounded-full border border-border"
        style={{ backgroundColor: tag.color || DEFAULT_COLOR }}
      />
      {canRead ? (
        <Link
          to={tagUrl(tag.id)}
          className="inline-flex items-center gap-1 rounded-sm hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          {name}
        </Link>
      ) : (
        <span className="inline-flex items-center gap-1">{name}</span>
      )}
      {inherited ? (
        <span className="font-normal text-muted-foreground">
          {" "}
          {inheritedFrom ? (
            <Link
              to={inheritedFrom.href}
              aria-label={`from table ${inheritedFrom.label}`}
              className="rounded-sm underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              from table
            </Link>
          ) : (
            "from table"
          )}
        </span>
      ) : null}
      {onRemove && !inherited ? (
        <IconButton aria-label={`Remove ${tag.name} from ${objectLabel}`} className="-mr-1 size-6" onClick={onRemove}>
          <X aria-hidden className="size-3" />
        </IconButton>
      ) : null}
    </Badge>
  );
}
