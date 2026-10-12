import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Check, ChevronDown, ChevronRight, Copy, FileOutput, FileText, Info, MoreHorizontal } from "lucide-react";
import type { TaskRunDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { FileRowList } from "@/components/workflows/executions/files/FileRow";
import { fileName } from "@/components/workflows/executions/files/fileModel";
import {
  copyText,
  findFileUris,
  isFileUri,
  LOOP_ITERATION_NOTE,
  outputExpression,
  previewValue,
  type OutputPath,
} from "@/components/workflows/executions/outputs/outputsModel";

/** A container with at most this many children starts open at the top level; a bigger one starts closed. */
const OPEN_AT_START_MAX = 50;
/**
 * How many children of one container are drawn, and how many more each "Show
 * more" adds. An opened 100k-item array draws 200 items, not 100k.
 */
export const CHILDREN_PAGE = 200;

interface TreeNode {
  id: string;
  label: string;
  depth: number;
  parentId: string | null;
  posInSet: number;
  setSize: number;
  /** A value's node; absent on a "Show more" node. */
  path?: OutputPath;
  value?: unknown;
  /** Children, when the value is a non-empty object or array. */
  childCount: number;
  /** A "Show more" node: the container it extends, and the first child it reveals. */
  more?: { containerId: string; nextId: string };
}

const nodeId = (path: OutputPath) => JSON.stringify(path);
const ROOT_ID = nodeId([]);

function children(value: unknown): [string | number, unknown][] {
  if (Array.isArray(value)) return value.map((v, i) => [i, v]);
  if (value !== null && typeof value === "object") return Object.entries(value);
  return [];
}

const childCount = (value: unknown) =>
  Array.isArray(value) ? value.length : value !== null && typeof value === "object" ? Object.keys(value).length : 0;

/**
 * The items a reader can see, in order: every node whose ancestors are all
 * open, at most `shown(container)` children per container, then a "Show
 * more" node for the rest.
 */
function visibleNodes(
  outputs: Record<string, unknown>,
  expanded: ReadonlySet<string>,
  shown: ReadonlyMap<string, number>,
): TreeNode[] {
  const out: TreeNode[] = [];
  const walk = (value: unknown, path: OutputPath, depth: number, containerId: string) => {
    const kids = children(value);
    const limit = shown.get(containerId) ?? CHILDREN_PAGE;
    const drawn = Math.min(kids.length, limit);
    const setSize = drawn < kids.length ? drawn + 1 : drawn;
    const parentId = containerId === ROOT_ID ? null : containerId;
    for (let i = 0; i < drawn; i++) {
      const [key, v] = kids[i];
      const p = [...path, key];
      const id = nodeId(p);
      const n = childCount(v);
      out.push({ id, path: p, label: typeof key === "number" ? `[${key}]` : key, value: v, depth, childCount: n, parentId, posInSet: i + 1, setSize });
      if (n > 0 && expanded.has(id)) walk(v, p, depth + 1, id);
    }
    if (drawn < kids.length) {
      const rest = kids.length - drawn;
      out.push({
        id: `${containerId}#more`,
        label: `Show ${Math.min(CHILDREN_PAGE, rest)} more of ${rest}`,
        depth,
        childCount: 0,
        parentId,
        posInSet: setSize,
        setSize,
        more: { containerId, nextId: nodeId([...path, kids[drawn][0]]) },
      });
    }
  };
  walk(outputs, [], 1, ROOT_ID);
  return out;
}

/** The top-level containers small enough to open at first, as Kestra's tree opens its first level. */
function initiallyExpanded(outputs: Record<string, unknown>): Set<string> {
  return new Set(
    Object.entries(outputs)
      .filter(([, v]) => {
        const n = childCount(v);
        return n > 0 && n <= OPEN_AT_START_MAX;
      })
      .map(([k]) => nodeId([k])),
  );
}

/** One line of the tree: a file shows its name, anything else a one-line preview. */
const nodePreview = (value: unknown) => (isFileUri(value) ? `file ${fileName(value)}` : previewValue(value));

type CopyStatus = { tone: "ok" | "error"; text: string } | null;

interface OutputsTreeProps {
  ws: string;
  executionId: string;
  run: TaskRunDto;
  /** The execution's `loop_value`: set on a `Loop` iteration's child execution. */
  loopValue?: string | null;
}

/**
 * One task run's outputs as a collapsible tree (W7b spec §8), with the
 * selected value's expression and two copy actions under it, then every file
 * in the outputs as a file row. Kestra's `ExecutionVariableExplorer.vue`: a
 * tree of the run's outputs whose selection gives the `{{ outputs.… }}` path,
 * and a copy button; its expression debugger is left out (spec §2).
 *
 * `run.outputs` is `null` for a run that has none (a `Log`, a `Sleep`, any run
 * not yet terminal), which is the commonest shape of all, so it is read as
 * empty here, where every consumer of the tree passes through.
 */
export function OutputsTree(props: OutputsTreeProps) {
  const outputs = props.run.outputs ?? {};
  if (Object.keys(outputs).length === 0) {
    // Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
    return <EmptyState icon={FileOutput} title="No outputs" description="This task run has no outputs." />;
  }
  // Mounted when outputs first appear, so a run whose outputs arrive on a later
  // poll opens its top level then, as one that had them from the start does.
  return <TreeBody {...props} outputs={outputs} />;
}

/**
 * The tree itself: the ARIA tree pattern (FE-A11Y-3). One tab stop (roving
 * `tabIndex`), the focused item is the selected one, Up/Down/Home/End move,
 * Right opens a container then enters it, Left closes it then goes to the
 * parent, and Enter or Space toggles, or shows more on a "Show more" item.
 * Each key costs O(1) lookups (an index by id), not a scan of the items. The
 * copy buttons sit outside the tree, so no item holds a control of its own.
 *
 * Mounted per run (the Gantt panel and the Outputs tab key it by `run.id`), so
 * what is open and selected starts afresh for another run, and survives a poll
 * that hands the same run back.
 */
function TreeBody({ ws, executionId, run, loopValue, outputs }: OutputsTreeProps & { outputs: Record<string, unknown> }) {
  const [expanded, setExpanded] = useState<Set<string>>(() => initiallyExpanded(outputs));
  const [shown, setShown] = useState<Map<string, number>>(() => new Map());
  const nodes = useMemo(() => visibleNodes(outputs, expanded, shown), [outputs, expanded, shown]);
  const indexOf = useMemo(() => new Map(nodes.map((n, i) => [n.id, i])), [nodes]);
  const [activeId, setActiveId] = useState<string | null>(null);
  // Set by a keyboard move; the render that follows focuses the new active item.
  const focusPending = useRef(false);
  const [status, setStatus] = useState<CopyStatus>(null);
  const items = useRef(new Map<string, HTMLLIElement>());
  const statusTimer = useRef<number | undefined>(undefined);
  const files = useMemo(
    () => findFileUris(outputs).map((f) => ({ key: nodeId(f.path), uri: f.uri, label: outputExpression(run.task_id, f.path) })),
    [outputs, run.task_id],
  );
  const filesId = useId();

  // The active item, or the first one when it has gone (closed under a parent, or a poll dropped it).
  const activeIndex = activeId === null ? undefined : indexOf.get(activeId);
  const active = nodes[activeIndex ?? 0];

  useEffect(() => {
    if (!focusPending.current || !active) return;
    focusPending.current = false;
    items.current.get(active.id)?.focus();
  });

  useEffect(() => () => window.clearTimeout(statusTimer.current), []);

  const moveTo = (node: TreeNode | undefined) => {
    // Already active: no render follows, so a pending focus would wait for an
    // unrelated one (a poll) and pull focus back from wherever the user went.
    if (!node || node.id === active?.id) return;
    focusPending.current = true;
    setActiveId(node.id);
  };
  const toggle = (node: TreeNode, open?: boolean) => {
    if (node.childCount === 0) return;
    setExpanded((prev) => {
      const next = new Set(prev);
      if (open ?? !prev.has(node.id)) next.add(node.id);
      else next.delete(node.id);
      return next;
    });
  };
  /** A "Show more" item: draw the next page, and move to its first child (the item itself goes away). */
  const showMore = (node: TreeNode) => {
    if (!node.more) return;
    const { containerId, nextId } = node.more;
    setShown((prev) => new Map(prev).set(containerId, (prev.get(containerId) ?? CHILDREN_PAGE) + CHILDREN_PAGE));
    focusPending.current = true;
    setActiveId(nextId);
  };
  const activate = (node: TreeNode) => (node.more ? showMore(node) : toggle(node));

  const onKeyDown = (e: KeyboardEvent<HTMLLIElement>, node: TreeNode) => {
    const i = indexOf.get(node.id) ?? 0;
    const isOpen = expanded.has(node.id);
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        moveTo(nodes[i + 1]);
        break;
      case "ArrowUp":
        moveTo(nodes[i - 1]);
        break;
      case "Home":
        moveTo(nodes[0]);
        break;
      case "End":
        moveTo(nodes[nodes.length - 1]);
        break;
      case "ArrowRight":
        if (node.childCount > 0 && !isOpen) toggle(node, true);
        else if (node.childCount > 0) moveTo(nodes[i + 1]);
        break;
      case "ArrowLeft":
        if (node.childCount > 0 && isOpen) toggle(node, false);
        else if (node.parentId !== null) moveTo(nodes[indexOf.get(node.parentId) ?? -1]);
        break;
      case "Enter":
      case " ":
        activate(node);
        break;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  };

  const copy = async (text: string, what: "value" | "expression") => {
    window.clearTimeout(statusTimer.current);
    try {
      if (!navigator.clipboard) throw new Error("no clipboard");
      await navigator.clipboard.writeText(text);
      setStatus({ tone: "ok", text: `Copied the ${what}.` });
    } catch {
      setStatus({
        tone: "error",
        text: `The ${what} could not be copied: the browser did not allow it. Select it above and copy it by hand.`,
      });
    }
    statusTimer.current = window.setTimeout(() => setStatus(null), 4000);
  };

  const selectedValue = active && !active.more && active.path ? active : undefined;
  const expression = selectedValue?.path ? outputExpression(run.task_id, selectedValue.path) : "";

  return (
    <div className="grid min-w-0 gap-3">
      {loopValue != null ? (
        <p className="flex items-start gap-2 text-sm text-muted-foreground">
          <Info aria-hidden className="mt-0.5 size-4 shrink-0" />
          <span>
            This execution is the loop iteration <span className="font-mono text-xs">{loopValue}</span>. These expressions
            resolve inside this iteration only. {LOOP_ITERATION_NOTE}
          </span>
        </p>
      ) : null}
      <ul role="tree" aria-label={`Outputs of ${run.task_id}`} className="max-h-96 overflow-auto rounded-md border py-1 font-mono text-xs">
        {nodes.map((node) => {
          const isOpen = expanded.has(node.id);
          const selected = node.id === active?.id;
          const preview = node.more ? "" : nodePreview(node.value);
          return (
            <li
              key={node.id}
              ref={(el) => {
                if (el) items.current.set(node.id, el);
                else items.current.delete(node.id);
              }}
              role="treeitem"
              aria-label={node.more ? node.label : `${node.label}, ${preview}`}
              aria-level={node.depth}
              aria-posinset={node.posInSet}
              aria-setsize={node.setSize}
              aria-expanded={node.childCount > 0 ? isOpen : undefined}
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              onClick={() => {
                setActiveId(node.id);
                activate(node);
              }}
              onKeyDown={(e) => onKeyDown(e, node)}
              onFocus={() => setActiveId(node.id)}
              className={`flex min-w-0 cursor-pointer items-center gap-1 py-0.5 pr-2 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset ${
                selected ? "bg-muted" : ""
              }`}
              style={{ paddingLeft: `${(node.depth - 1) * 1 + 0.25}rem` }}
            >
              <span aria-hidden className="inline-flex size-4 shrink-0 items-center justify-center text-muted-foreground">
                {node.more ? (
                  <MoreHorizontal className="size-3.5" />
                ) : node.childCount > 0 ? (
                  isOpen ? (
                    <ChevronDown className="size-3.5" />
                  ) : (
                    <ChevronRight className="size-3.5" />
                  )
                ) : null}
              </span>
              {node.more ? (
                <span className="text-muted-foreground tabular-nums">{node.label}</span>
              ) : (
                <>
                  <span className="shrink-0 font-medium">{node.label}</span>
                  <span aria-hidden className="text-muted-foreground">
                    :
                  </span>
                  {isFileUri(node.value) ? <FileText aria-hidden className="size-3.5 shrink-0 text-muted-foreground" /> : null}
                  <span className="min-w-0 truncate text-muted-foreground" title={typeof node.value === "string" ? node.value : undefined}>
                    {preview}
                  </span>
                </>
              )}
            </li>
          );
        })}
      </ul>
      {selectedValue ? (
        <section aria-label="Selected output" className="grid gap-2">
          <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
            <span className="text-muted-foreground">Expression</span>
            <code data-testid="output-expression" className="min-w-0 break-all rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
              {expression}
            </code>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="outline" size="sm" onClick={() => void copy(copyText(selectedValue.value), "value")}>
              <Copy aria-hidden />
              Copy value
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => void copy(expression, "expression")}>
              <Copy aria-hidden />
              Copy expression
            </Button>
            <span role="status" className={`text-sm ${status?.tone === "error" ? "text-destructive" : "text-muted-foreground"}`}>
              {status ? (
                <span className="inline-flex items-center gap-1">
                  {status.tone === "ok" ? <Check aria-hidden className="size-4" /> : null}
                  {status.text}
                </span>
              ) : null}
            </span>
          </div>
        </section>
      ) : null}
      {files.length > 0 ? (
        <section aria-labelledby={filesId} className="grid gap-2">
          <span id={filesId} className="text-sm font-medium">
            Files
          </span>
          <FileRowList ws={ws} executionId={executionId} files={files} />
        </section>
      ) : null}
    </div>
  );
}
