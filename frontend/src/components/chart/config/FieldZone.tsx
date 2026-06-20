import {
  DndContext, PointerSensor, useSensor, useSensors, closestCenter, type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, verticalListSortingStrategy, useSortable, arrayMove,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Plus } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

function Chip({ id, onRemove }: { id: string; onRemove: () => void }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className="flex items-center gap-1.5 rounded-md border bg-muted/40 px-2 py-1 text-sm"
    >
      <button type="button" className="cursor-grab text-muted-foreground" {...attributes} {...listeners} aria-label="drag">
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <span className="flex-1">{id}</span>
      <button type="button" aria-label={`remove ${id}`} className="text-muted-foreground hover:text-destructive" onClick={onRemove}>×</button>
    </div>
  );
}

export function FieldZone({
  label, selected, options, onChange,
}: { label: string; selected: string[]; options: string[]; onChange: (v: string[]) => void }) {
  const remaining = options.filter((o) => !selected.includes(o));
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
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
              <Chip key={s} id={s} onRemove={() => onChange(selected.filter((x) => x !== s))} />
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
          <PopoverContent align="start" className="w-48 p-1">
            <div className="flex flex-col">
              {remaining.map((o) => (
                <button key={o} type="button" className="rounded px-2 py-1.5 text-left text-sm hover:bg-muted"
                  onClick={() => onChange([...selected, o])}>{o}</button>
              ))}
            </div>
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
