import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, verticalListSortingStrategy, useSortable, arrayMove,
  sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Plus, X } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { GROUP_LABEL } from "@/components/chart/settings/FieldLabel";

function Chip({ id, label, onRemove }: { id: string; label: string; onRemove: () => void }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className="flex min-h-8 items-center gap-1 rounded-md border bg-muted/40 pr-0.5 pl-1 text-sm"
    >
      {/* The drag handle carries dnd-kit's keyboard listeners as well as its pointer ones, so
          reordering works from the keyboard once this button has focus (space to lift, arrows to
          move, space to drop). `aria-label` names the field, not just "drag" — a screen-reader
          user tabbing through four identical "drag" buttons cannot tell which row they are on.

          THE ICON STAYS 14px AND THE TARGET GROWS AROUND IT. Measured live in the pivot's panel,
          this button was 14×14 and the × beside it was 8×20 — against the 24×24 in WCAG 2.2 SC
          2.5.8, in a chip that had 200px of inert space between them. `size-6` is 24×24 with the
          same glyph centred inside, so nothing looks heavier and both are hittable. */}
      <button type="button"
        className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        {...attributes} {...listeners}
        aria-label={`reorder ${label}`}>
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      {/* The friendly name, with the machine name still available on hover — the field is chosen by
          what it means, but a SQL-facing user may need the real column. */}
      <span className="min-w-0 flex-1 truncate" title={label === id ? id : `${label} (${id})`}>{label}</span>
      <button type="button" aria-label={`remove ${label}`}
        className="flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        onClick={onRemove}>
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

export function FieldZone({
  label, selected, options, onChange, labels,
}: {
  label: string;
  selected: string[];
  options: string[];
  onChange: (v: string[]) => void;
  /** Machine name -> display name (a dataset column's `verbose_name`, or a metric's). Datasets
   * have carried these for a long time and offer a UI to set them, but the chart studio read only
   * the machine name, so naming a column "Total Sales" changed nothing here. Absent or blank falls
   * back to the machine name. */
  labels?: Record<string, string>;
}) {
  const nameOf = (n: string) => labels?.[n]?.trim() || n;
  const remaining = options.filter((o) => !selected.includes(o));
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    // Chart 03 found the table's column reorder was mouse-only and fixed it there; this zone was
    // missed and had the same defect.
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  function onDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const from = selected.indexOf(String(active.id));
    const to = selected.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    onChange(arrayMove(selected, from, to));
  }
  // The zone's chips are a NAMED GROUP, not a loose run of buttons: without this a screen-reader
  // user tabbing the pivot's panel hears "reorder order_month", "remove order_month", "add rows…"
  // with nothing anywhere saying which zone any of it belongs to — and the three zones are
  // otherwise indistinguishable, since the heading was a bare `<div>` associated with nothing.
  const groupId = `zone-${label.toLowerCase().replace(/\s+/g, "-")}`;
  return (
    <div className="space-y-1.5" role="group" aria-labelledby={groupId}>
      <div id={groupId} className={GROUP_LABEL}>{label}</div>
      {/* A zone with no chips and nothing left to add rendered its heading over EMPTY SPACE — seen
          live on the pivot's Values zone against a dataset that defines no metrics, where the
          "Add values…" button is suppressed by `remaining.length > 0` and the chip list is empty,
          so the panel read "VALUES" and then nothing at all. Say why it is empty instead. */}
      {selected.length === 0 && remaining.length === 0 && (
        <p className="text-xs italic text-muted-foreground">
          Nothing available for {label.toLowerCase()}.
        </p>
      )}
      <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
        <SortableContext items={selected} strategy={verticalListSortingStrategy}>
          <div className="flex flex-col gap-1">
            {selected.map((s) => (
              <Chip key={s} id={s} label={nameOf(s)} onRemove={() => onChange(selected.filter((x) => x !== s))} />
            ))}
          </div>
        </SortableContext>
      </DndContext>
      {remaining.length > 0 && (
        <Popover>
          <PopoverTrigger asChild>
            {/* `min-h-8` for the same reason the chip's two buttons grew: this was 26px tall, under
                the 24×24 minimum only in one axis but sitting directly beneath 32px rows, so the
                zone's own rhythm broke at its most-clicked control. */}
            <button type="button" aria-label={`add ${label.toLowerCase()}`}
              className="flex min-h-8 w-full items-center gap-1 rounded-md border border-dashed px-2 text-xs text-muted-foreground hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset">
              <Plus className="h-3.5 w-3.5" /> Add {label.toLowerCase()}…
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-56 p-1">
            <div className="flex flex-col">
              {remaining.map((o) => (
                <button key={o} type="button"
                  className="flex min-h-8 flex-col justify-center rounded px-2 py-1 text-left text-sm hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
                  onClick={() => onChange([...selected, o])}>
                  <span className="block truncate">{nameOf(o)}</span>
                  {nameOf(o) !== o && <span className="block truncate text-xs text-muted-foreground">{o}</span>}
                </button>
              ))}
            </div>
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
