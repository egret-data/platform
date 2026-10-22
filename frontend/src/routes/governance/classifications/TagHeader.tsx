import { useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Lock, Pencil } from "lucide-react";
import { toast } from "sonner";
import { type Tag, useTags, useUpdateTag } from "@/api/tags";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import {
  ColourField,
  DEFAULT_COLOR,
  NameTaken,
  nameHolder,
  TagSwatch,
  useCanManageTags,
} from "@/routes/governance/classifications/tagParts";
import { tableTagMaskConflict, tagNameTaken } from "@/routes/governance/apiErrors";
import { RequiredMark } from "@/routes/governance/tabParts";
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

/**
 * The inline edit form: only changed fields are sent; a refusal keeps the form
 * and its edits (FE-ACTION-3). A rename onto another classification's name
 * (case-insensitive) is said under Name before Save, which then waits; the
 * name as it stands is never checked, so a duplicate pair that already exists
 * does not block editing the other fields (the server checks only a real
 * rename). The server's `tag_name_taken` and `table_tag_mask_conflict` sit
 * under Name too; anything else over the form.
 */
function EditForm({ tag, onDone }: { tag: Tag; onDone: () => void }) {
  const update = useUpdateTag();
  const tags = useTags();
  const [name, setName] = useState(tag.name);
  const [color, setColor] = useState(tag.color || DEFAULT_COLOR);
  const [description, setDescription] = useState(tag.description);
  const [serverNameError, setServerNameError] = useState<ReactNode>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = { name: useId(), nameError: useId(), color: useId(), description: useId(), waits: useId() };

  const changes: { name?: string; color?: string; description?: string } = {};
  if (name.trim() !== tag.name) changes.name = name.trim();
  if (color !== (tag.color || DEFAULT_COLOR)) changes.color = color;
  if (description.trim() !== tag.description) changes.description = description.trim();
  const dirty = Object.keys(changes).length > 0;
  useUnsavedChangesWarning(dirty);

  const holder = changes.name !== undefined ? nameHolder(tags.data, changes.name, tag.id) : null;
  const nameError: ReactNode = serverNameError ?? (holder ? <NameTaken holder={holder} /> : null);
  const empty = !name.trim();
  const waiting = empty || !!nameError || update.isPending;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (waiting) return;
    if (!dirty) {
      onDone();
      return;
    }
    setError(null);
    setServerNameError(null);
    update.mutate(
      { id: tag.id, ...changes },
      {
        onSuccess: () => {
          toast.success("Classification saved.");
          onDone();
        },
        onError: (err) => {
          const taken = tagNameTaken(err);
          const conflict = tableTagMaskConflict(err);
          if (taken) setServerNameError(<NameTaken holder={taken.existing} />);
          else if (conflict) {
            setServerNameError(
              `${conflict.tag} is masked by ${conflict.mask} while it sits on the whole table ${conflict.schema}.${conflict.table}; attach it to columns first.`,
            );
          } else setError(err.message || "The classification could not be saved. Try again.");
        },
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
      <div className="space-y-1.5">
        <Label htmlFor={ids.name}>
          Name
          <RequiredMark />
        </Label>
        <Input
          id={ids.name}
          value={name}
          aria-required="true"
          autoFocus
          aria-invalid={nameError ? true : undefined}
          aria-describedby={nameError ? ids.nameError : undefined}
          onChange={(e) => {
            setName(e.target.value);
            setServerNameError(null);
          }}
        />
        {nameError ? (
          <p id={ids.nameError} className="text-sm text-destructive">
            {nameError}
          </p>
        ) : null}
      </div>
      <ColourField id={ids.color} color={color} onChange={setColor} />
      <div className="space-y-1.5">
        <Label htmlFor={ids.description}>Description</Label>
        <Textarea id={ids.description} rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      <p className="text-xs text-muted-foreground">
        {TERM.key} <span className="font-mono">{tag.key}</span> is fixed once created.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="submit"
          size="sm"
          aria-disabled={waiting ? true : undefined}
          aria-describedby={empty ? ids.waits : nameError ? ids.nameError : undefined}
        >
          Save
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onDone}>
          Cancel
        </Button>
        {empty ? (
          <p id={ids.waits} className="text-xs text-muted-foreground">
            Enter a name.
          </p>
        ) : null}
      </div>
    </form>
  );
}
