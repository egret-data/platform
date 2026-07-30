import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, verticalListSortingStrategy, useSortable, arrayMove,
  sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Plus } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

function Chip({ id, label, onRemove }: { id: string; label: string; onRemove: () => void }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className="flex items-center gap-1.5 rounded-md border bg-muted/40 px-2 py-1 text-sm"
    >
      {/* The drag handle carries dnd-kit's keyboard listeners as well as its pointer ones, so
          reordering works from the keyboard once this button has focus (space to lift, arrows to
          move, space to drop). `aria-label` names the field, not just "drag" — a screen-reader
          user tabbing through four identical "drag" buttons cannot tell which row they are on. */}
      <button type="button" className="cursor-grab text-muted-foreground" {...attributes} {...listeners}
        aria-label={`reorder ${label}`}>
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      {/* The friendly name, with the machine name still available on hover — the field is chosen by
          what it means, but a SQL-facing user may need the real column. */}
      <span className="flex-1 truncate" title={label === id ? id : `${label} (${id})`}>{label}</span>
      <button type="button" aria-label={`remove ${label}`} className="text-muted-foreground hover:text-destructive" onClick={onRemove}>×</button>
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
  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</div>
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
            <button type="button" aria-label={`add ${label.toLowerCase()}`}
              className="flex w-full items-center gap-1 rounded-md border border-dashed px-2 py-1 text-xs text-muted-foreground hover:bg-muted/40">
              <Plus className="h-3.5 w-3.5" /> Add {label.toLowerCase()}…
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-56 p-1">
            <div className="flex flex-col">
              {remaining.map((o) => (
                <button key={o} type="button" className="rounded px-2 py-1.5 text-left text-sm hover:bg-muted"
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
