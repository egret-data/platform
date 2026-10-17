import { KeyRound, Tag as TagIcon, UserSearch } from "lucide-react";
import type { TagDto } from "@/api/generated/model";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { dottedPath, KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { engineTypeLabel, type EngineInfo } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/** What the header's tag section shows; `null` for an object that cannot carry tags (no section at all). */
export type HeaderTags = null | { state: "loading" } | { state: "error" } | { state: "ok"; tags: TagDto[] };

/**
 * What the header says when the tags could not be read. Also this module's
 * bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const TAGS_FAILED = "Tags could not be loaded.";

/** The kind of an object its schema's listing could not tell. */
export const KIND_UNKNOWN = "Kind unknown";

/**
 * The header's kind: an object's comes from its schema's listing, so it is
 * `"loading"` until that answers and `"unknown"` when it cannot (the listing
 * failed, was not listed, or does not name the object).
 */
export type HeaderKind = NodeKind | "loading" | "unknown";

/**
 * Each action, the permission its own route requires, and its label. Grant
 * access writes `POST /perm/grants`, gated on `policy:update`
 * (`crates/egret-perm-compile/src/routes.rs`, `create_grant`). Add tag
 * writes `PUT …/resource-tags`, gated on `catalog_registry:update`
 * (`crates/egret-bi/src/catalog_tags.rs`, `WAREHOUSE_WRITE`).
 */
const ACTIONS = [
  { key: "onGrant", permission: "policy:update", label: "Grant access", icon: KeyRound },
  { key: "onAddTag", permission: "catalog_registry:update", label: "Add tag", icon: TagIcon },
  { key: "onWhoCanRead", permission: "policy:read", label: "Who can read?", icon: UserSearch },
] as const;

type ActionKey = (typeof ACTIONS)[number]["key"];

/**
 * An object's header (spec §2.2): its name, kind and path, the engines its
 * catalog is attached to, its own tags, and its actions.
 *
 * **Tags.** Only tables and columns carry tags (migration 0177), so a
 * catalog or schema header has no tag section, and neither has an object in a
 * catalog that is not egret's (F9). A table's header shows the tags on the
 * table itself; a column's own tags and the ones it inherits from its table
 * are on the column's row.
 *
 * **Actions.** Each shows only when its handler is wired AND the viewer holds
 * its permission; one named in `disabledReasons` shows disabled, with the
 * reason beside it. The page owns the dialogs they open (`GrantDialog`,
 * `AddTagDialog`, `WhoCanReadDialog`); a page that does not pass a handler
 * shows no button for it.
 */
export function ObjectHeader({
  path,
  kind,
  engines,
  tags,
  onGrant,
  onAddTag,
  onWhoCanRead,
  disabledReasons = {},
}: {
  path: ObjectPath;
  kind: HeaderKind;
  engines: EngineInfo[];
  tags: HeaderTags;
  onGrant?: () => void;
  onAddTag?: () => void;
  onWhoCanRead?: () => void;
  /**
   * An action shown but disabled, with the reason said beside it
   * (FE-ACTION-4): Add tag on an object outside egret's catalogs (F9). It
   * still shows only with its handler and its permission.
   */
  disabledReasons?: Partial<Record<ActionKey, string>>;
}) {
  const can = usePolicyPermissions();
  const handlers = { onGrant, onAddTag, onWhoCanRead };
  const actions = ACTIONS.filter((a) => handlers[a.key] && can(a.permission));
  const name = path.object ?? path.schema ?? path.catalog;
  const kindMeta = kind === "loading" || kind === "unknown" ? null : KIND_ICON[kind];

  return (
    <section aria-label="Object" className="space-y-3 border-b border-border pb-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          {/* Takes focus when the page opens from the folded tree, or the column panel closes with no opener (D6, D7). */}
          <h1 tabIndex={-1} className="text-2xl font-semibold break-all outline-none">
            {name}
          </h1>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
            {kindMeta ? (
              <span className="inline-flex items-center gap-1">
                <kindMeta.icon aria-hidden className="size-4" />
                {kindMeta.label}
              </span>
            ) : kind === "loading" ? (
              <>
                <Skeleton className="h-4 w-16" />
                <span className="sr-only">Loading kind</span>
              </>
            ) : (
              <span>{KIND_UNKNOWN}</span>
            )}
            <span className="font-mono text-xs break-all">{dottedPath(path)}</span>
          </div>
        </div>
        {actions.length > 0 ? (
          <div className="space-y-1">
            <div className="flex flex-wrap gap-2">
              {actions.map((a) => {
                const reason = disabledReasons[a.key];
                return (
                  <Button
                    key={a.key}
                    variant="outline"
                    size="sm"
                    onClick={handlers[a.key]}
                    disabled={reason !== undefined}
                    aria-describedby={reason !== undefined ? `object-action-${a.key}-reason` : undefined}
                  >
                    <a.icon aria-hidden />
                    {a.label}
                  </Button>
                );
              })}
            </div>
            {actions.map((a) =>
              disabledReasons[a.key] !== undefined ? (
                <p key={a.key} id={`object-action-${a.key}-reason`} className="text-xs text-muted-foreground">
                  {disabledReasons[a.key]}
                </p>
              ) : null,
            )}
          </div>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="text-xs text-muted-foreground">Engines</span>
        {engines.length === 0 ? (
          <span className="text-xs text-muted-foreground">No engine attached</span>
        ) : (
          <ul aria-label="Engines" className="flex flex-wrap gap-1">
            {engines.map((e) => (
              <li key={e.name}>
                <Badge variant="outline">{engineText(e)}</Badge>
              </li>
            ))}
          </ul>
        )}
      </div>

      {tags ? (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="text-xs text-muted-foreground">Tags</span>
          {tags.state === "loading" ? (
            <>
              <Skeleton className="h-5 w-24" />
              <span className="sr-only">Loading tags</span>
            </>
          ) : tags.state === "error" ? (
            <span className="text-xs text-destructive">{TAGS_FAILED}</span>
          ) : tags.tags.length === 0 ? (
            <span className="text-xs text-muted-foreground">None on this object</span>
          ) : (
            <ul aria-label="Tags" className="flex flex-wrap gap-1">
              {tags.tags.map((t) => (
                <li key={t.id}>
                  <Badge variant="secondary" title={t.key}>
                    {t.name}
                  </Badge>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}

/**
 * `trino-1 · Trino`, with the status when the engine is not running and the
 * origin when egret does not run it (an external engine's status is a
 * placeholder, migration 0184, so it is not shown).
 */
function engineText(e: EngineInfo): string {
  const parts = [e.name];
  if (e.type) parts.push(engineTypeLabel(e.type));
  if (e.origin === "external") parts.push("external");
  else if (e.status && e.status !== "running") parts.push(e.status);
  return parts.join(" · ");
}
