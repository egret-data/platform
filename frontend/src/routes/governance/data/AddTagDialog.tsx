import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, Tag as TagIcon } from "lucide-react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { tagsQueryKey } from "@/api/tags";
import { useControlPlaneCatalogAssignResourceTag } from "@/api/generated/control-plane-catalog/control-plane-catalog";
import { getControlPlaneGovernanceObjectTagsQueryKey } from "@/api/generated/control-plane-governance/control-plane-governance";
import { getControlPlaneTagsListTagsQueryKey, useControlPlaneTagsListTags } from "@/api/generated/control-plane-tags/control-plane-tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { refusalMessage } from "@/routes/governance/policyWrites";
import { fullPath } from "@/routes/governance/data/objectData";
import { failedText } from "@/routes/governance/data/tabs/TabStates";

/**
 * Why Add tag is disabled on an object outside egret's catalogs (F9: the tag
 * API is keyed by egret's warehouse, so only an egret catalog can carry a
 * tag). Also this module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const TAGS_EGRET_ONLY = "Tags can be added to egret-managed catalogs only";

/** What the tag picker says when `GET /tags` is refused (it needs `tag:read`, which writing a tag does not imply). */
const TAG_LIST_NEEDS_TAG_READ = "Reading the tag list needs the tag:read permission.";

/** The table or column a tag is added to. */
export type TagTarget = {
  catalog: string;
  catalogId: string;
  warehouseId: string;
  schema: string;
  table: string;
  /** Absent for the table itself. */
  column?: string;
  /** The ids of the tags already on this table or column, which are not offered again. */
  assigned: string[];
};

/**
 * Add one tag to a table or a column (spec §2.2, F9) through the existing
 * resource-tags write, `PUT /warehouses/{warehouse_id}/resource-tags`, gated
 * on `catalog_registry:update` (`crates/egret-bi/src/catalog_tags.rs`,
 * `WAREHOUSE_WRITE`). The picker offers the tag catalogue (`GET /tags`,
 * `tag:read`) less the tags already on the target.
 *
 * On success it refetches the object's tags (the header, the Overview's
 * columns, the column panel and the tag rules all read them) and the tag
 * catalogue (its assignment counts) under both keys it is cached by, the
 * generated `GET /tags` key and `tagsQueryKey` (`api/tags.ts`), closes and
 * toasts. On failure the
 * server's sentence shows beside the field and the choice is kept: a 409
 * names the two rules the tag would make overlap, a 400 the level a
 * coordinate cannot be stored at.
 */
export function AddTagDialog({
  target,
  onOpenChange,
}: {
  /** `null` while closed. */
  target: TagTarget | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={target !== null} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        {target ? <AddTagForm target={target} onDone={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function AddTagForm({ target, onDone }: { target: TagTarget; onDone: () => void }) {
  const [tagId, setTagId] = useState("");
  const tagsQ = useControlPlaneTagsListTags({ query: { retry: false } });
  const queryClient = useQueryClient();
  const assign = useControlPlaneCatalogAssignResourceTag<ApiError>({
    mutation: {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: [getControlPlaneGovernanceObjectTagsQueryKey(target.catalogId)[0]] });
        void queryClient.invalidateQueries({ queryKey: getControlPlaneTagsListTagsQueryKey() });
        // Classifications, TagDetail, RuleDialog and the rule lists read the catalogue under this key.
        void queryClient.invalidateQueries({ queryKey: tagsQueryKey });
        toast.success("Tag added.");
        onDone();
      },
    },
  });

  const path = fullPath({ catalog: target.catalog, schema: target.schema, object: target.table, column: target.column });
  const assigned = new Set(target.assigned);
  const offered =
    tagsQ.data?.status === 200
      ? tagsQ.data.data.filter((t) => !assigned.has(t.id)).sort((a, b) => a.name.localeCompare(b.name))
      : undefined;
  const noun = target.column === undefined ? "table" : "column";

  let picker;
  if (tagsQ.isError) {
    const refused = tagsQ.error instanceof ApiError && tagsQ.error.status === 403;
    picker = (
      <Alert variant="destructive">
        <AlertDescription>{refused ? TAG_LIST_NEEDS_TAG_READ : failedText("tag list")}</AlertDescription>
      </Alert>
    );
  } else if (offered === undefined) {
    picker = <ListSkeleton rows={1} cols={1} />;
  } else if (offered.length === 0) {
    picker = (
      <p className="text-sm text-muted-foreground">
        {assigned.size > 0 && tagsQ.data?.status === 200 && tagsQ.data.data.length > 0
          ? `Every tag is already on this ${noun}.`
          : "No tags are defined yet."}
      </p>
    );
  } else {
    picker = (
      <div className="space-y-1.5">
        <Label htmlFor="add-tag-tag">Tag</Label>
        <Select value={tagId} onValueChange={setTagId}>
          <SelectTrigger id="add-tag-tag" className="w-full" aria-describedby={assign.isError ? "add-tag-error" : undefined}>
            <SelectValue placeholder="Pick a tag" />
          </SelectTrigger>
          <SelectContent>
            {offered.map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {assign.isError ? (
          <p id="add-tag-error" role="alert" className="text-sm text-destructive">
            {refusalMessage(assign.error, "catalog_registry:update", "The tag could not be added. Try again.")}
          </p>
        ) : null}
      </div>
    );
  }

  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!tagId) return;
        assign.mutate({
          warehouseId: target.warehouseId,
          data: { schema: target.schema, table: target.table, column: target.column ?? "", tag_id: tagId },
        });
      }}
    >
      <DialogHeader>
        <DialogTitle>Add tag</DialogTitle>
        <DialogDescription>
          <span className="font-mono text-xs break-all">{path}</span>
        </DialogDescription>
      </DialogHeader>
      {picker}
      <DialogFooter className="items-center">
        {offered && offered.length > 0 && !tagId ? (
          <p id="add-tag-needs" className="text-xs text-muted-foreground">
            Pick a tag to add.
          </p>
        ) : null}
        <Button
          type="submit"
          disabled={!tagId || assign.isPending}
          aria-describedby={offered && offered.length > 0 && !tagId ? "add-tag-needs" : undefined}
        >
          {assign.isPending ? <Loader2 aria-hidden className="animate-spin" /> : <TagIcon aria-hidden />}
          Add tag
        </Button>
      </DialogFooter>
    </form>
  );
}
