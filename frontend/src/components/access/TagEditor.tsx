/**
 * Classification tag chips + editor for a BI object (datasets first).
 *
 * `TagChips` is the read-only presentation, reusable anywhere an object's
 * tags are displayed (e.g. a list row or detail header). `TagEditor` is the
 * assign/unassign UI: current tags as removable chips plus an "Add tag"
 * select sourced from the org-wide catalog (`useTags`), filtered to tags not
 * already on the object. `TagDialog` wraps `TagEditor` in a `Dialog`,
 * mirroring `ShareDialog`'s per-object modal pattern.
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
import { useAssignTag, useObjectTags, useTags, useUnassignTag, type Tag } from "@/api/tags";

/** Small colored dot representing a tag's catalog color. */
function TagSwatch({ color }: { color: string }) {
  return (
    <span
      aria-hidden="true"
      className="h-2 w-2 shrink-0 rounded-full"
      style={{ backgroundColor: color }}
    />
  );
}

/**
 * Read-only tag chips. Renders nothing (a muted em dash) when there are no
 * tags, so it's safe to drop into any info panel or table cell.
 */
export function TagChips({ tags }: { tags: Tag[] }) {
  if (!tags || tags.length === 0) {
    return <span className="text-sm text-muted-foreground">—</span>;
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.map((tag) => (
        <Badge key={tag.id} variant="outline" className="gap-1 font-normal">
          <TagSwatch color={tag.color} />
          {tag.name}
        </Badge>
      ))}
    </div>
  );
}

/** "Add tag" select: catalog tags not already assigned to the object. */
function AddTagSelect({
  ws,
  kind,
  id,
  available,
}: {
  ws: string;
  kind: string;
  id: string;
  available: Tag[];
}) {
  const assign = useAssignTag(ws, kind, id);
  // Kept as a controlled value only so the trigger resets to its placeholder
  // after each pick; the select isn't meant to "stay" on a chosen tag.
  const [value, setValue] = useState("");

  return (
    <Select
      value={value}
      onValueChange={(tagId) => {
        assign.mutate(tagId);
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
  );
}

/**
 * Assign/unassign editor for an object's tags. Current tags render as
 * removable chips; the "Add tag" select offers the rest of the catalog.
 */
export function TagEditor({ ws, kind, id }: { ws: string; kind: string; id: string }) {
  const objectTagsQ = useObjectTags(ws, kind, id);
  const catalogQ = useTags();
  const unassign = useUnassignTag(ws, kind, id);

  if (objectTagsQ.isLoading || catalogQ.isLoading) {
    return <p className="text-sm text-muted-foreground">…</p>;
  }

  const tags = objectTagsQ.data ?? [];
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
                onClick={() => unassign.mutate(tag.id)}
                className="ml-0.5 rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))
        )}
      </div>

      {catalog.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No tags defined — create them in Control Plane → Classifications.
        </p>
      ) : (
        <AddTagSelect ws={ws} kind={kind} id={id} available={available} />
      )}
    </div>
  );
}

interface TagDialogProps {
  ws: string;
  kind: string;
  id: string;
  /** Human name of the object, shown in the header. */
  name?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Metabase-style "Tags" modal for a single object, mirroring `ShareDialog`. */
export function TagDialog({ ws, kind, id, name, open, onOpenChange }: TagDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Tags{name ? ` for "${name}"` : ""}</DialogTitle>
          <DialogDescription>Classify this {kind} with tags.</DialogDescription>
        </DialogHeader>
        <TagEditor ws={ws} kind={kind} id={id} />
      </DialogContent>
    </Dialog>
  );
}
