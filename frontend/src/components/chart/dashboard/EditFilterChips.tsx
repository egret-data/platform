import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, horizontalListSortingStrategy, useSortable, sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { reorderIds } from "@/lib/reorderIds";

/** Metabase's own activation distance for this list (`ParametersList.tsx:51`,
 *  `useDndSensors({ distance: 15 })`). A chip is also a click target — it opens the filter's
 *  settings panel — so a drag must not start until the pointer has clearly committed. */
export const FILTER_DRAG_ACTIVATION_PX = 15;

/** Move `activeId` to the slot `overId` occupies. Same-reference no-op when the ids match or
 *  either is missing, so the caller can skip the draft write entirely. */
export function reorderParameters(
  parameters: DashboardParameter[], activeId: string, overId: string,
): DashboardParameter[] {
  const ids = parameters.map((p) => p.id);
  const next = reorderIds(ids, activeId, overId);
  if (next === ids) return parameters;
  const byId = new Map(parameters.map((p) => [p.id, p]));
  return next.map((id) => byId.get(id)!);
}

interface Props {
  parameters: DashboardParameter[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onReorder: (next: DashboardParameter[]) => void;
}

function Chip({ param, active, onSelect }: {
  param: DashboardParameter; active: boolean; onSelect: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: param.id });
  return (
    <div ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className={`inline-flex h-8 items-center rounded-sm border pr-2 text-sm ${active ? "border-primary bg-secondary text-primary" : "bg-background hover:bg-accent/40"}`}>
      {/* The handle carries dnd-kit's keyboard listeners as well as its pointer ones, so the order
          can be changed without a mouse (focus, space to lift, arrows to move, space to drop).
          `size-6` is the 24×24 minimum in WCAG 2.2 SC 2.5.8 around a 14px glyph — the same shape
          `FieldZone`'s chip settled on. The label names the FILTER: a run of identical "reorder"
          buttons is unusable from a screen reader. */}
      <button type="button" aria-label={`reorder ${param.name}`}
        className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        {...attributes} {...listeners}>
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <button type="button" aria-label={param.name} data-active={active}
        className="h-full" onClick={onSelect}>
        {param.name}{param.required ? " *" : ""}
      </button>
    </div>
  );
}

export function EditFilterChips({ parameters, activeId, onSelect, onReorder }: Props) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: FILTER_DRAG_ACTIVATION_PX } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  function onDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over) return;
    const next = reorderParameters(parameters, String(active.id), String(over.id));
    if (next !== parameters) onReorder(next);
  }
  if (parameters.length === 0) return null;
  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
      <SortableContext items={parameters.map((p) => p.id)} strategy={horizontalListSortingStrategy}>
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {parameters.map((p) => (
            <Chip key={p.id} param={p} active={p.id === activeId} onSelect={() => onSelect(p.id)} />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}
