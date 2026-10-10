/**
 * Classification tag editor for a catalog resource (a warehouse table, or a
 * specific column of it) — the catalog-browse counterpart of `TagEditor`.
 *
 * Where `TagEditor` keys off a BI object's uuid, this one keys off the
 * resource's coordinates (`warehouseId` + `schema` + `table` + optional
 * `column`) since catalog tables/columns aren't uuid-addressed objects.
 * `column` omitted (or `""`) means the table itself.
 */
import { useState } from "react";
import { X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useTags, type Tag } from "@/api/tags";
import { ApiError } from "@/api/client";
import { useAssignResourceTag, useResourceTags, useUnassignResourceTag } from "@/api/catalogTags";

/** Small colored dot representing a tag's catalog color — mirrors `TagEditor`'s. */
function TagSwatch({ color }: { color: string }) {
  return (
    <span
      aria-hidden="true"
      className="h-2 w-2 shrink-0 rounded-full"
      style={{ backgroundColor: color }}
    />
  );
}

interface ResourceCoords {
  warehouseId: string;
  schema: string;
  table: string;
  /** Omitted or `""` = the table itself. */
  column?: string;
}

/**
 * "Add tag" select: catalog tags not already assigned to this resource.
 *
 * ⚠ **The failure message is RENDERED, and that is load-bearing rather than
 * polish** (SP4d debt Task 3, fix round, Minor 1). Migration 0177 made some
 * coordinates unstorable and `assign_catalog` refuses them with a 400 naming the
 * offending level — but the trade that justified the refusal was "a loud 400 at
 * write time beats a silent decline at read time", and as first shipped this
 * component had no `onError` and discarded the mutation result, so a steward
 * tagging a table named `ord*ers` saw the trigger snap back to its placeholder and
 * **nothing else**: the tag silently was not applied. That is the exact failure
 * mode the refusal was supposed to trade away.
 *
 * `ApiError.from` puts the body's `error` field on `Error.message`, so the
 * backend's own sentence — which names the level — is what shows. Rendering
 * `assign.error` needs no `onError`: react-query already holds the last failure,
 * and a thrown-away `mutate()` result is precisely what went wrong here.
 */
function AddCatalogTagSelect({
  warehouseId,
  schema,
  table,
  column,
  available,
}: ResourceCoords & { available: Tag[] }) {
  const assign = useAssignResourceTag(warehouseId);
  // Kept as a controlled value only so the trigger resets to its placeholder
  // after each pick; the select isn't meant to "stay" on a chosen tag.
  const [value, setValue] = useState("");

  return (
    <>
      <Select
        value={value}
        onValueChange={(tagId) => {
          assign.mutate({ schema, table, column: column ?? "", tag_id: tagId });
          setValue("");
        }}
      >
        <SelectTrigger className="h-8 w-56" aria-label="Add tag">
          <SelectValue placeholder="Add tag…" />
        </SelectTrigger>
        <SelectContent>
          {available.map((tag) => (
            <SelectItem key={tag.id} value={tag.id}>
              <span className="flex items-center gap-2">
                <TagSwatch color={tag.color} />
                {tag.name}
              </span>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {assign.isError ? (
        <p
          className="text-sm text-destructive"
          role="alert"
          data-testid="catalog-tag-assign-error"
        >
          Could not add that tag: {assign.error.message}
        </p>
      ) : null}
    </>
  );
}

/**
 * Assign/unassign editor for a catalog table/column's tags. Current tags
 * render as removable chips; the "Add tag" select offers the rest of the
 * org-wide catalog.
 *
 * ⚠ **The REMOVE failure is rendered too** (SP4d debt Task 11). The mechanism is
 * identical to `AddCatalogTagSelect`'s: `unassign.mutate()` discarded its result and
 * the component carried no `onError`, so a failed removal left the chip sitting
 * there — `unassign`'s `onSuccess` is what invalidates the query, so on failure
 * nothing refetches and nothing changes. Indistinguishable from a slow refetch.
 *
 * **But the TRIGGER is not the assign half's 400, and saying so matters for what a
 * test may assert.** `unassign_catalog` deliberately carries NO
 * `unresolvable_coordinate` check (`crates/egret-bi/src/catalog_tags.rs`, the
 * "`unassign_catalog` deliberately has no such check" paragraph): a `DELETE` is not
 * constrained by migration 0177's `CHECK`, and a deployment holding a bad row must
 * be able to remove it through the API. So no wildcard coordinate can make this
 * mutation fail. What CAN, by code path:
 *
 * * **403, routinely reachable.** This dialog hangs off `BrowseTab`, inside a route
 *   gated `catalog_registry:read` (`App.tsx`'s `warehouses/:id`), and the Remove
 *   button is rendered for every chip with no permission check — while
 *   `unassign_resource_tag` requires `catalog_registry:update`. A steward with read
 *   but not update clicks Remove and, before this fix, saw nothing at all.
 * * **500.** `unassign_catalog` bumps every bound service's `tag_version` and
 *   recompiles the bound policy profiles *in the same transaction*; a recompile
 *   failure rolls the DELETE back and returns 500.
 * * network loss, and the 403/`feature_disabled` race `client.ts` documents.
 *
 * **One departure from the assign half, and it is because the measurement differs.**
 * Task 11's brief asked for "the backend's own sentence rather than a frontend copy",
 * which is right for assign: `unresolvable_coordinate` produces a sentence that NAMES
 * the offending level, and a UI paraphrase would be a fifth place for it to drift. For
 * unassign the dominant reachable failure is the 403 above, and its "own sentence" is
 * `Error::Forbidden`'s Display — the bare word **`forbidden`** (`egret-core/src/error.rs`;
 * there is no per-permission message to copy). Rendering only that would ship an alert
 * that is visible and still says nothing actionable. `BrowseTab` already settled this
 * exact question ~20 lines from the dialog's own trigger, for the READ 403 on the same
 * route: it names the permission. So this does the same — 403 names
 * `catalog_registry:update`, every other status shows the backend's message verbatim.
 * The verbatim arm is what carries a 500, a Conflict, or any future refusal with real
 * wording, so nothing has to be edited here when one appears.
 */
export function CatalogTagEditor({ warehouseId, schema, table, column }: ResourceCoords) {
  const resourceTagsQ = useResourceTags(warehouseId, schema, table);
  const catalogQ = useTags();
  const unassign = useUnassignResourceTag(warehouseId);

  if (resourceTagsQ.isLoading || catalogQ.isLoading) {
    return <p className="text-sm text-muted-foreground">…</p>;
  }

  const key = column ?? "";
  const tags = (resourceTagsQ.data ?? [])
    .filter((a) => (a.column ?? "") === key)
    .map((a) => a.tag);
  const catalog = catalogQ.data ?? [];
  const assignedIds = new Set(tags.map((t) => t.id));
  const available = catalog.filter((t) => !assignedIds.has(t.id));

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-1.5" aria-label="Assigned tags">
        {tags.length === 0 ? (
          <span className="text-sm text-muted-foreground">No tags assigned.</span>
        ) : (
          tags.map((tag) => (
            <Badge key={tag.id} variant="outline" className="gap-1 py-0.5 pr-1 font-normal">
              <TagSwatch color={tag.color} />
              {tag.name}
              <button
                type="button"
                aria-label={`Remove ${tag.name}`}
                onClick={() => unassign.mutate({ schema, table, column: key, tag_id: tag.id })}
                className="ml-0.5 rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))
        )}
      </div>

      {unassign.isError ? (
        <p
          className="text-sm text-destructive"
          role="alert"
          data-testid="catalog-tag-unassign-error"
        >
          Could not remove that tag:{" "}
          {unassign.error instanceof ApiError && unassign.error.status === 403
            ? "this needs the catalog_registry:update permission. A platform administrator can grant it."
            : unassign.error.message}
        </p>
      ) : null}

      {catalog.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No tags defined — create them in Governance → Classifications.
        </p>
      ) : (
        <AddCatalogTagSelect
          warehouseId={warehouseId}
          schema={schema}
          table={table}
          column={column}
          available={available}
        />
      )}
    </div>
  );
}

interface CatalogTagDialogProps extends ResourceCoords {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Metabase-style "Tags" modal for a catalog table or column. */
export function CatalogTagDialog({
  warehouseId,
  schema,
  table,
  column,
  open,
  onOpenChange,
}: CatalogTagDialogProps) {
  const resourceLabel = column ? `${schema}.${table}.${column}` : `${schema}.${table}`;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Tags for &ldquo;{resourceLabel}&rdquo;</DialogTitle>
          <DialogDescription>
            Classify this {column ? "column" : "table"} with tags.
          </DialogDescription>
        </DialogHeader>
        <CatalogTagEditor warehouseId={warehouseId} schema={schema} table={table} column={column} />
      </DialogContent>
    </Dialog>
  );
}
