import { createContext, useContext, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import type { Node as FlowNode, NodeProps } from "@xyflow/react";
import { ChevronsDownUp, ChevronsUpDown, Expand } from "lucide-react";
import type { Cluster, NodeState } from "@/api/generated/model";
import { IconButton } from "@/components/ui/icon-button";
import { StateChip } from "@/components/workflows/StateChip";
import type { Positioned } from "@/components/workflows/topology/layout";
import {
  BRANCH_LOOK,
  CLUSTER_WORD,
  REFUSAL_NOTE,
  attemptsText,
  clusterIcon,
  clusterKindOf,
  durationText,
  iterationsText,
  nodeFacts,
  shortType,
  typeIcon,
  type NodeFacts,
  type TaskNode,
  type TriggerNode,
} from "@/components/workflows/topology/topologyModel";
import { cn } from "@/lib/utils";

/**
 * THE CANVAS'S NODES (W7c spec §6.1). Each card holds one focusable `<button>`
 * (the node), and at most one action beside it (Expand a `Subflow`, expand a
 * collapsed cluster), never inside it: a button cannot hold a button.
 *
 * The node buttons share ONE tab stop (`tabStop`), moved by the arrow keys in
 * the layout's order (FE-A11Y-3); the view owns that order. Colours are
 * tokens: classes, or `var(--token)` in inline styles (FE-VISUAL-1/2). A
 * branch's colour always comes with its word ("On error", "Finally"), so it is
 * never colour alone (FE-A11Y-4).
 */

export interface TopologyContextValue {
  states?: Readonly<Record<string, NodeState>>;
  clusters: readonly Cluster[];
  selectedUid: string | null;
  /** The one node button in the tab order. */
  tabStop: string | null;
  /** The clock a running node's duration is measured to. */
  now: number;
  onSelect: (uid: string) => void;
  onExpandSubflow: (uid: string) => void;
  toggleCluster: (clusterUid: string) => void;
  onNodeKeyDown: (e: KeyboardEvent<HTMLButtonElement>, id: string) => void;
  onNodeFocus: (id: string) => void;
  register: (id: string, el: HTMLButtonElement | null) => void;
}

export const TopologyContext = createContext<TopologyContextValue | null>(null);

function useTopology(): TopologyContextValue {
  const ctx = useContext(TopologyContext);
  if (!ctx) throw new Error("A topology node is drawn outside TopologyView.");
  return ctx;
}

export type TopologyFlowNode = FlowNode<{ item: Positioned }>;

/** The flowable task a cluster stands for (`cluster:{owner}`), when it has one: the triggers cluster has none. */
export function clusterOwner(c: Cluster): string | null {
  const owner = c.uid.slice("cluster:".length);
  return c.nodes.includes(owner) ? owner : null;
}

/**
 * xyflow sets `pointer-events: none` on a node it neither drags nor selects,
 * and the property is inherited, so a card takes pointer events back for its
 * buttons. Inline rather than a class, so jsdom (no stylesheet) sees it too.
 */
const TAKE_POINTER: CSSProperties = { pointerEvents: "auto" };

/** The hatched border of a task egret refuses to run: a gradient, so a `var(--token)` in a style. */
const REFUSED_HATCH: CSSProperties = {
  backgroundImage: "repeating-linear-gradient(135deg, var(--destructive) 0 4px, transparent 4px 8px)",
};

const NODE_BUTTON = cn(
  "flex h-full w-full min-w-0 flex-col items-start gap-0 rounded-md px-2.5 py-2 text-left",
  "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
);

function NodeButton({ id, label, onActivate, children }: { id: string; label?: string; onActivate: () => void; children: ReactNode }) {
  const ctx = useTopology();
  const selected = ctx.selectedUid !== null && (id === ctx.selectedUid || label === ctx.selectedUid);
  return (
    <button
      type="button"
      data-uid={id}
      ref={(el) => ctx.register(id, el)}
      tabIndex={ctx.tabStop === id ? 0 : -1}
      aria-current={selected || undefined}
      onClick={onActivate}
      onFocus={() => ctx.onNodeFocus(id)}
      onKeyDown={(e) => ctx.onNodeKeyDown(e, id)}
      className={cn(NODE_BUTTON, "nodrag nopan")}
    >
      {children}
    </button>
  );
}

function Line({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn("block h-4 w-full truncate text-xs leading-4", className)}>{children}</span>;
}

/** The lines under a card's title; `factLines` counts them for the layout, in the same order. */
function Facts({ facts, now }: { facts: NodeFacts; now: number }) {
  const state = facts.state;
  const duration = state ? durationText(state, now) : null;
  const attempts = state ? attemptsText(state) : null;
  return (
    <>
      {facts.badges.length > 0 ? <Line className="text-muted-foreground">{facts.badges.join(" · ")}</Line> : null}
      {facts.refusal ? (
        <>
          <Line className="font-medium">{REFUSAL_NOTE}</Line>
          <Line className="font-mono text-muted-foreground">{facts.refusal}</Line>
        </>
      ) : null}
      {facts.expansion ? (
        <span className="line-clamp-2 h-8 w-full text-xs leading-4 text-muted-foreground">{facts.expansion}</span>
      ) : null}
      {state ? (
        <>
          <span className="mt-1 flex h-5 w-full items-center gap-2">
            <StateChip state={state.state} kind="execution" />
            {duration ? <span className="truncate text-xs text-muted-foreground tabular-nums">{duration}</span> : null}
          </span>
          {state.iterations ? <Line className="tabular-nums">{iterationsText(state.iterations)}</Line> : null}
          {attempts ? <Line className="text-muted-foreground tabular-nums">{attempts}</Line> : null}
        </>
      ) : null}
    </>
  );
}

/** A task or a trigger. */
export function TaskCard({ data }: NodeProps<TopologyFlowNode>) {
  const ctx = useTopology();
  const item = data.item;
  const node = item.node as TaskNode | TriggerNode;
  const facts = nodeFacts(node, ctx.states, item.expansion);
  const { icon: Icon, name } = typeIcon(node, node.node === "task" ? clusterKindOf(node.uid, ctx.clusters) : undefined);
  const branch = node.node === "task" ? BRANCH_LOOK[node.branch] : undefined;
  const id = node.node === "task" ? node.task_id : node.trigger_id;
  // A `Subflow` that has no cluster yet was not asked to expand; a failed one has a cluster with a reason.
  const canExpand = node.node === "task" && node.kind === "executable" && !ctx.clusters.some((c) => c.uid === `cluster:${node.uid}`);
  return (
    <div
      data-node-card
      data-dimmed={facts.dimmed || undefined}
      data-refused={facts.refusal ? true : undefined}
      className={cn(
        "relative h-full w-full rounded-md",
        facts.refusal ? "p-1" : "border-2 border-border",
        facts.dimmed && "opacity-60",
        ctx.selectedUid === node.uid && "outline-2 outline-offset-2 outline-primary",
      )}
      style={{ ...TAKE_POINTER, ...(facts.refusal ? REFUSED_HATCH : { borderColor: branch?.colour }) }}
    >
      <div className={cn("h-full w-full rounded-md bg-card text-card-foreground", facts.refusal && "rounded-sm")}>
        <NodeButton id={node.uid} onActivate={() => ctx.onSelect(node.uid)}>
          <span className={cn("flex h-6 w-full min-w-0 items-center gap-1.5", canExpand && "pr-7")}>
            <Icon data-icon={name} aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
            <span className="truncate font-mono text-sm font-medium" title={id}>
              {id}
            </span>
            <span className="ml-auto shrink-0 pl-1 text-xs text-muted-foreground">{shortType(node.type)}</span>
          </span>
          <Facts facts={facts} now={ctx.now} />
        </NodeButton>
      </div>
      {canExpand ? (
        <IconButton
          aria-label={`Expand subflow ${id}`}
          title="Draw the child flow here"
          onClick={() => ctx.onExpandSubflow(node.uid)}
          className="nodrag nopan absolute top-2.5 right-2"
        >
          <Expand aria-hidden className="size-4" />
        </IconButton>
      ) : null}
    </div>
  );
}

/** The flow's or a cluster's start and end: a dot, not a stop for the keyboard. */
export function MarkerDot({ data }: NodeProps<TopologyFlowNode>) {
  const start = data.item.kind === "start";
  return (
    <div
      aria-hidden
      data-marker={data.item.id}
      className={cn("size-full rounded-full border-2 border-muted-foreground", start ? "bg-muted-foreground" : "bg-background")}
    />
  );
}

function clusterName(c: Cluster): string {
  return c.task_id ?? CLUSTER_WORD[c.kind].toLowerCase();
}

/** An expanded cluster: a box behind its members, with a header that collapses it. */
export function ClusterBox({ data }: NodeProps<TopologyFlowNode>) {
  const ctx = useTopology();
  const c = data.item.cluster as Cluster;
  const name = clusterName(c);
  return (
    <div data-cluster={c.uid} className="size-full rounded-lg border border-border bg-muted/40">
      <div style={TAKE_POINTER} className="flex h-8 items-center gap-1.5 px-2 text-xs text-muted-foreground">
        <span className="truncate font-medium text-foreground">{CLUSTER_WORD[c.kind]}</span>
        {c.task_id ? <span className="truncate font-mono">{c.task_id}</span> : null}
        <IconButton
          aria-label={`Collapse ${name}`}
          aria-expanded
          title="Collapse"
          onClick={() => ctx.toggleCluster(c.uid)}
          className="nodrag nopan ml-auto"
        >
          <ChevronsDownUp aria-hidden className="size-4" />
        </IconButton>
      </div>
    </div>
  );
}

/** A collapsed cluster: one card standing for the flowable and everything in it. */
export function CollapsedCard({ data }: NodeProps<TopologyFlowNode>) {
  const ctx = useTopology();
  const c = data.item.cluster as Cluster;
  const owner = clusterOwner(c);
  const name = clusterName(c);
  const icon = clusterIcon(c.kind);
  const Icon = icon.icon;
  const state = ctx.states && owner ? (ctx.states[owner] ?? null) : undefined;
  return (
    <div
      data-node-card
      data-collapsed={c.uid}
      style={TAKE_POINTER}
      className={cn(
        "relative h-full w-full rounded-md border-2 border-border bg-card text-card-foreground shadow-sm",
        owner !== null && ctx.selectedUid === owner && "outline-2 outline-offset-2 outline-primary",
      )}
    >
      <NodeButton id={c.uid} label={owner ?? undefined} onActivate={() => (owner ? ctx.onSelect(owner) : ctx.toggleCluster(c.uid))}>
        <span className="flex h-6 w-full min-w-0 items-center gap-1.5 pr-7">
          <Icon data-icon={icon.name} aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate font-mono text-sm font-medium">{name}</span>
          <span className="ml-auto shrink-0 pl-1 text-xs text-muted-foreground">{CLUSTER_WORD[c.kind]}</span>
        </span>
        {state ? (
          <span className="mt-1 flex h-5 items-center">
            <StateChip state={state.state} kind="execution" />
          </span>
        ) : state === null ? (
          <Line className="mt-1 text-muted-foreground">Not run</Line>
        ) : null}
      </NodeButton>
      <IconButton
        aria-label={`Expand ${name}`}
        aria-expanded={false}
        title="Expand"
        onClick={() => ctx.toggleCluster(c.uid)}
        className="nodrag nopan absolute top-2.5 right-2"
      >
        <ChevronsUpDown aria-hidden className="size-4" />
      </IconButton>
    </div>
  );
}

/** `@xyflow/react`'s node types; a constant, so the canvas never sees a new object. */
export const NODE_TYPES = {
  task: TaskCard,
  marker: MarkerDot,
  cluster: ClusterBox,
  collapsed: CollapsedCard,
};
