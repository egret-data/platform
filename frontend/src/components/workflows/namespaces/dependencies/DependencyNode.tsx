import { createContext, useContext, type CSSProperties, type KeyboardEvent } from "react";
import { Link } from "react-router-dom";
import type { Node as XyNode, NodeProps } from "@xyflow/react";
import { Workflow } from "lucide-react";
import { dynamicSubflowsText, type FlowBox } from "@/components/workflows/namespaces/dependencies/dependenciesModel";
import { cn } from "@/lib/utils";

/**
 * A FLOW'S CARD on the namespace dependencies canvas (W7d spec §4.2): its id, its namespace, its
 * states in words, and its dynamic subflows. The card is one link to the flow (W7a's flow route),
 * so selecting it, Enter on it, or opening it in a new tab all open that flow.
 *
 * The links share ONE tab stop (`tabStop`), moved by the arrow keys in the layout's order
 * (FE-A11Y-3), as W7c's topology cards are. A flow outside the namespace is muted, and its card
 * says "Outside this namespace" (never colour alone, FE-A11Y-4).
 */

export interface DependenciesContextValue {
  /** The flow page a card opens: `flowHref(id)`. */
  flowHref: (id: string) => string;
  /** The one card in the tab order. */
  tabStop: string | null;
  onKeyDown: (e: KeyboardEvent<HTMLAnchorElement>, id: string) => void;
  onFocus: (id: string) => void;
  register: (id: string, el: HTMLAnchorElement | null) => void;
}

export const DependenciesContext = createContext<DependenciesContextValue | null>(null);

export type FlowCardNode = XyNode<{ box: FlowBox }>;

/** xyflow sets `pointer-events: none` on a node it neither drags nor selects; the card takes them back for its link. */
const TAKE_POINTER: CSSProperties = { pointerEvents: "auto" };

export function FlowCard({ data }: NodeProps<FlowCardNode>) {
  const ctx = useContext(DependenciesContext);
  if (!ctx) throw new Error("A dependency card is drawn outside the dependencies graph.");
  const { box } = data;
  const f = box.flow;
  const dynamic = dynamicSubflowsText(f.dynamic_subflows);
  return (
    <div
      data-flow-card
      data-outside={f.in_namespace ? undefined : true}
      className={cn(
        "h-full w-full rounded-md border-2",
        f.unparsable ? "border-dashed border-destructive" : "border-border",
        f.in_namespace ? "bg-card text-card-foreground" : "bg-muted text-muted-foreground",
        f.disabled && "opacity-60",
      )}
      style={TAKE_POINTER}
    >
      <Link
        to={ctx.flowHref(f.id)}
        data-flow={f.id}
        ref={(el) => ctx.register(f.id, el)}
        tabIndex={ctx.tabStop === f.id ? 0 : -1}
        onFocus={() => ctx.onFocus(f.id)}
        onKeyDown={(e) => ctx.onKeyDown(e, f.id)}
        className={cn(
          "nodrag nopan flex h-full w-full min-w-0 flex-col items-start rounded-md px-2.5 py-2 text-left hover:bg-muted/60",
          "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
        )}
      >
        <span className="flex h-6 w-full min-w-0 items-center gap-1.5">
          <Workflow aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate font-mono text-sm font-medium" title={f.flow_id}>
            {f.flow_id}
          </span>
        </span>
        <span className="block h-4 w-full truncate text-xs leading-4 text-muted-foreground" title={f.namespace}>
          {f.namespace}
        </span>
        {box.badges.map((b) => (
          <span key={b} className="block h-4 w-full truncate text-xs leading-4">
            {b}
          </span>
        ))}
        {dynamic ? <span className="block h-4 w-full truncate text-xs leading-4 tabular-nums text-muted-foreground">{dynamic}</span> : null}
      </Link>
    </div>
  );
}

export const NODE_TYPES = { flow: FlowCard };
