import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { ReactNode } from "react";

/**
 * A sortable row that hands its drag handle back to the caller.
 *
 * `useSortable` is a hook, so a row that wants one has to be its own component — and the rows that
 * want one here (a series, a pie slice) carry five or six controls each. Extracting them properly
 * would mean threading every one of those through a props object; this passes the two things the
 * caller actually needs (`attributes`, `listeners`) into the existing JSX and leaves the rest where
 * it is.
 *
 * The handle is deliberately NOT spread on the row itself. dnd-kit will happily make a whole row
 * draggable, and then a click on the colour swatch or the rename input starts a drag instead of
 * doing what it says.
 */
export function SortableRow({ id, className, children }: {
  id: string;
  className?: string;
  children: (handle: {
    attributes: ReturnType<typeof useSortable>["attributes"];
    listeners: ReturnType<typeof useSortable>["listeners"];
  }) => ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className={className}
    >
      {children({ attributes, listeners })}
    </div>
  );
}
