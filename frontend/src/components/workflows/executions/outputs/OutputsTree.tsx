import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Check, ChevronDown, ChevronRight, Copy, FileOutput, FileText, Info } from "lucide-react";
import type { TaskRunDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { FileRow } from "@/components/workflows/executions/files/FileRow";
import { fileName } from "@/components/workflows/executions/files/fileModel";
import {
  copyText,
  findFileUris,
  isFileUri,
  LOOP_EXPRESSION_NOTE,
  outputExpression,
  outputPath,
  previewValue,
  type OutputPath,
} from "@/components/workflows/executions/outputs/outputsModel";

/** A container with at most this many children starts open at the top level; a bigger one starts closed. */
const OPEN_AT_START_MAX = 50;

interface TreeNode {
  id: string;
  path: OutputPath;
  label: string;
  value: unknown;
  depth: number;
  /** Children, when the value is a non-empty object or array. */
  childCount: number;
  parentId: string | null;
  posInSet: number;
  setSize: number;
}

const nodeId = (path: OutputPath) => JSON.stringify(path);

function children(value: unknown): [string | number, unknown][] {
  if (Array.isArray(value)) return value.map((v, i) => [i, v]);
  if (value !== null && typeof value === "object") return Object.entries(value);
  return [];
}

/** The items a reader can see, in order: every node whose ancestors are all open. */
function visibleNodes(outputs: Record<string, unknown>, expanded: ReadonlySet<string>): TreeNode[] {
  const out: TreeNode[] = [];
  const walk = (value: unknown, path: OutputPath, depth: number, parentId: string | null) => {
    const kids = children(value);
    kids.forEach(([key, v], i) => {
      const p = [...path, key];
      const id = nodeId(p);
      const childCount = children(v).length;
      out.push({
        id,
        path: p,
        label: typeof key === "number" ? `[${key}]` : key,
        value: v,
        depth,
        childCount,
        parentId,
        posInSet: i + 1,
        setSize: kids.length,
      });
      if (childCount > 0 && expanded.has(id)) walk(v, p, depth + 1, id);
    });
  };
  walk(outputs, [], 1, null);
  return out;
}

/** The top-level containers small enough to open at first, as Kestra's tree opens its first level. */
function initiallyExpanded(outputs: Record<string, unknown>): Set<string> {
  return new Set(
    Object.entries(outputs)
      .filter(([, v]) => {
        const n = children(v).length;
        return n > 0 && n <= OPEN_AT_START_MAX;
      })
      .map(([k]) => nodeId([k])),
  );
}

/** One line of the tree: a file shows its name, anything else a one-line preview. */
const nodePreview = (value: unknown) => (isFileUri(value) ? `file ${fileName(value)}` : previewValue(value));

type CopyStatus = { tone: "ok" | "error"; text: string } | null;

/**
 * One task run's outputs as a collapsible tree (W7b spec §8), with the
 * selected value's expression and two copy actions under it, then every file
 * in the outputs as a file row. Kestra's `ExecutionVariableExplorer.vue`: a
 * tree of the run's outputs whose selection gives the `{{ outputs.… }}` path,
 * and a copy button; its expression debugger is left out (spec §2).
 *
 * The tree is the ARIA tree pattern (FE-A11Y-3): one tab stop (roving
 * `tabIndex`), the focused item is the selected one, Up/Down/Home/End move,
 * Right opens a container then enters it, Left closes it then goes to the
 * parent, and Enter or Space toggles. The copy buttons sit outside the tree,
 * so no item holds a control of its own.
 *
 * Mounted per run (the Gantt panel and the Outputs tab key it by `run.id`),
 * so what is open and selected starts afresh for another run, and survives a
 * poll that hands the same run back.
 */
export function OutputsTree({ ws, executionId, run }: { ws: string; executionId: string; run: TaskRunDto }) {
  const outputs = run.outputs;
  const [expanded, setExpanded] = useState<Set<string>>(() => initiallyExpanded(outputs));
  const nodes = useMemo(() => visibleNodes(outputs, expanded), [outputs, expanded]);
  const [activeId, setActiveId] = useState<string | null>(null);
  // Set by a keyboard move; the render that follows focuses the new active item.
  const focusPending = useRef(false);
  const [status, setStatus] = useState<CopyStatus>(null);
  const items = useRef(new Map<string, HTMLLIElement>());
  const statusTimer = useRef<number | undefined>(undefined);
  const files = useMemo(() => findFileUris(outputs), [outputs]);
  const value = run.value ?? null;
  const filesId = useId();

  // The active item, or the first one when it has gone (closed under a parent, or a poll dropped it).
  const active = nodes.find((n) => n.id === activeId) ?? nodes[0];

  useEffect(() => {
    if (!focusPending.current || !active) return;
    focusPending.current = false;
    items.current.get(active.id)?.focus();
  });

  useEffect(() => () => window.clearTimeout(statusTimer.current), []);

  if (Object.keys(outputs).length === 0) {
    // Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
    return <EmptyState icon={FileOutput} title="No outputs" description="This task run has no outputs." />;
  }

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
      const isOpen = prev.has(node.id);
      if (open ?? !isOpen) next.add(node.id);
      else next.delete(node.id);
      return next;
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLLIElement>, node: TreeNode) => {
    const i = nodes.findIndex((n) => n.id === node.id);
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
        else moveTo(nodes.find((n) => n.id === node.parentId));
        break;
      case "Enter":
      case " ":
        toggle(node);
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

  const expression = active ? outputExpression(run.task_id, value, active.path) : "";

  return (
    <div className="grid min-w-0 gap-3">
      {value !== null ? (
        <p className="flex items-start gap-2 text-sm text-muted-foreground">
          <Info aria-hidden className="mt-0.5 size-4 shrink-0" />
          <span>
            {`${LOOP_EXPRESSION_NOTE}, in Kestra's outputs.<taskId>[value] form: `}
            <code className="font-mono text-xs">{outputPath(run.task_id, value, [])}</code>.
          </span>
        </p>
      ) : null}
      <ul role="tree" aria-label={`Outputs of ${run.task_id}`} className="max-h-96 overflow-auto rounded-md border py-1 font-mono text-xs">
        {nodes.map((node) => {
          const isOpen = expanded.has(node.id);
          const selected = node.id === active?.id;
          const preview = nodePreview(node.value);
          return (
            <li
              key={node.id}
              ref={(el) => {
                if (el) items.current.set(node.id, el);
                else items.current.delete(node.id);
              }}
              role="treeitem"
              aria-label={`${node.label}, ${preview}`}
              aria-level={node.depth}
              aria-posinset={node.posInSet}
              aria-setsize={node.setSize}
              aria-expanded={node.childCount > 0 ? isOpen : undefined}
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              onClick={() => {
                setActiveId(node.id);
                toggle(node);
              }}
              onKeyDown={(e) => onKeyDown(e, node)}
              onFocus={() => setActiveId(node.id)}
              className={`flex min-w-0 cursor-pointer items-center gap-1 py-0.5 pr-2 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset ${
                selected ? "bg-muted" : ""
              }`}
              style={{ paddingLeft: `${(node.depth - 1) * 1 + 0.25}rem` }}
            >
              <span aria-hidden className="inline-flex size-4 shrink-0 items-center justify-center text-muted-foreground">
                {node.childCount > 0 ? isOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" /> : null}
              </span>
              <span className="shrink-0 font-medium">{node.label}</span>
              <span aria-hidden className="text-muted-foreground">
                :
              </span>
              {isFileUri(node.value) ? <FileText aria-hidden className="size-3.5 shrink-0 text-muted-foreground" /> : null}
              <span className="min-w-0 truncate text-muted-foreground" title={typeof node.value === "string" ? node.value : undefined}>
                {preview}
              </span>
            </li>
          );
        })}
      </ul>
      {active ? (
        <section aria-label="Selected output" className="grid gap-2">
          <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
            <span className="text-muted-foreground">Expression</span>
            <code data-testid="output-expression" className="min-w-0 break-all rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
              {expression}
            </code>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="outline" size="sm" onClick={() => void copy(copyText(active.value), "value")}>
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
          <ul className="grid gap-2">
            {files.map((f) => (
              <li key={nodeId(f.path)}>
                <FileRow ws={ws} executionId={executionId} uri={f.uri} label={outputExpression(run.task_id, value, f.path)} />
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
