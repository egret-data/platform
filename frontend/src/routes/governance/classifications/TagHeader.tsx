import { useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Lock, Pencil } from "lucide-react";
import { toast } from "sonner";
import { type Tag, useUpdateTag } from "@/api/tags";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { DEFAULT_COLOR, TagSwatch, useCanManageTags } from "@/routes/governance/classifications/tagParts";
import { TERM } from "@/routes/governance/glossary";

/**
 * A classification's header (spec §5): swatch, name (the page's `h1`), key,
 * description and, when gated, the word Clearance with a lock (FE-A11Y-4).
 * With `tag:manage` or platform admin, Edit opens an inline form (name,
 * colour, description) saved with Save, not on blur; `actions` holds the
 * page's other controls (Delete).
 */
export function TagHeader({ tag, actions }: { tag: Tag; actions?: ReactNode }) {
  const canManage = useCanManageTags();
  const [editing, setEditing] = useState(false);
  // Closing the form unmounts the control that had focus; Edit takes it back
  // when it mounts again (live check D5), as a dialog returns focus to its opener.
  const refocusEdit = useRef(false);
  const closeForm = () => {
    refocusEdit.current = true;
    setEditing(false);
  };
  const gated = tag.required_roles.length + tag.required_groups.length > 0;

  return (
    <section aria-label={TERM.classification} className="space-y-3">
      {editing ? (
        <EditForm tag={tag} onDone={closeForm} />
      ) : (
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 items-start gap-3">
            <TagSwatch color={tag.color} size="lg" />
            <div className="min-w-0 space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <h1 className="break-words text-2xl font-semibold">{tag.name}</h1>
                <Badge variant="secondary" className="font-mono text-xs">
                  {tag.key}
                </Badge>
                {gated ? (
                  <Badge variant="outline" className="gap-1">
                    <Lock className="h-3 w-3" aria-hidden />
                    Clearance
                  </Badge>
                ) : null}
              </div>
              {tag.description ? <p className="text-sm text-muted-foreground">{tag.description}</p> : null}
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            {canManage ? (
              <Button
                size="sm"
                variant="outline"
                ref={(el) => {
                  if (el && refocusEdit.current) {
                    refocusEdit.current = false;
                    el.focus();
                  }
                }}
                onClick={() => setEditing(true)}
              >
                <Pencil className="mr-1 h-4 w-4" aria-hidden />
                Edit
              </Button>
            ) : null}
            {actions}
          </div>
        </div>
      )}
    </section>
  );
}

/** The inline edit form: only changed fields are sent; a refusal keeps the form and its edits (FE-ACTION-3). */
function EditForm({ tag, onDone }: { tag: Tag; onDone: () => void }) {
  const update = useUpdateTag();
  const [name, setName] = useState(tag.name);
  const [color, setColor] = useState(tag.color || DEFAULT_COLOR);
  const [description, setDescription] = useState(tag.description);
  const [error, setError] = useState<string | null>(null);
  const ids = { name: useId(), color: useId(), description: useId() };

  const changes: { name?: string; color?: string; description?: string } = {};
  if (name.trim() !== tag.name) changes.name = name.trim();
  if (color !== (tag.color || DEFAULT_COLOR)) changes.color = color;
  if (description.trim() !== tag.description) changes.description = description.trim();
  const dirty = Object.keys(changes).length > 0;
  useUnsavedChangesWarning(dirty);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!dirty) {
      onDone();
      return;
    }
    setError(null);
    update.mutate(
      { id: tag.id, ...changes },
      {
        onSuccess: () => {
          toast.success("Classification saved.");
          onDone();
        },
        onError: (err) => setError(err.message || "The classification could not be saved. Try again."),
      },
    );
  };

  return (
    <form onSubmit={submit} className="max-w-xl space-y-4" aria-label="Edit classification">
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <div className="flex items-end gap-3">
        <div className="flex-1 space-y-1.5">
          <Label htmlFor={ids.name}>Name</Label>
          <Input id={ids.name} value={name} required autoFocus onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={ids.color}>Colour</Label>
          <Input
            id={ids.color}
            type="color"
            className="h-9 w-12 p-1"
            value={color}
            onChange={(e) => setColor(e.target.value)}
          />
        </div>
      </div>
      <div className="space-y-1.5">
        <Label htmlFor={ids.description}>Description</Label>
        <Textarea id={ids.description} rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      <p className="text-xs text-muted-foreground">
        Key <span className="font-mono">{tag.key}</span> is fixed once created.
      </p>
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={update.isPending || !name.trim()}>
          Save
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
