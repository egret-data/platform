import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "@/api/client";
import { workflowGetFlowGraph, workflowGraphUnsavedFlow } from "@/api/generated/workflow/workflow";
import type { FlowGraph } from "@/api/generated/model";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { rangeOfPath } from "@/components/workflows/editor/yamlPath";
import { TopologyView } from "@/components/workflows/topology/TopologyView";

/** Spec §6.2: the graph is redrawn 1 s after the last edit. */
export const TOPOLOGY_DEBOUNCE_MS = 1000;

/** The last answer: a graph, a YAML that does not parse (400), or a refusal or failure. */
type Answer = { key: string } & ({ kind: "ok" } | { kind: "unparsable" | "error"; message: string });

/**
 * What the tab keeps across a remount. The frame remounts a tab whenever the
 * set of panels changes (W7a §5.1, Mounting), so the last graph and the
 * expanded Subflows of the one flow open now live here rather than only in the
 * tab, which would otherwise come back as a skeleton with every Subflow folded.
 */
let remembered: { owner: string; graph: FlowGraph | null; expanded: string[] } | null = null;

function messageOf(err: unknown): string {
  return err instanceof Error && err.message ? err.message : "The request did not complete.";
}

/**
 * THE EDITOR'S TOPOLOGY TAB (W7c spec §6.2): the graph of the YAML in the
 * editor, saved or not, drawn by {@link TopologyView}. Read-only: selecting a
 * node moves the Code tab's cursor to its task, nothing more.
 *
 * - **Updates.** `POST flows/graph` with the editor's source, 1 s after the
 *   last edit; at once when the tab opens or a Subflow is expanded. A newer
 *   request aborts the older. A viewer without `workflow:manage` (which that
 *   route checks) cannot edit, so their tab draws the saved draft, the text the
 *   editor shows them, through `GET flows/{id}/graph`.
 * - **Errors.** While the YAML does not parse (400), the last graph stays,
 *   under §6.2's banner; with no graph to keep, the parse error is shown with
 *   its line. Any other failure (`graph_too_large`, the network) keeps the
 *   graph and says why (FE-STATE-1).
 * - **Expansion.** The expanded Subflows are the tab's, not the source's: an
 *   edit keeps them, and the server ignores a uid that names no Subflow.
 *
 * This module is the Topology chunk's entry from the editor: the editor's
 * registration (`registerEditorTabs.tsx`) reaches it through `lazy()` only.
 */
export function TopologyTab({ ctx }: { ctx: EditorContext }) {
  const { workspace, modelKey, readOnly, source, focusRange } = ctx;
  // A flow not created yet has no id to remember it by.
  const owner = modelKey === "new" ? null : `${workspace}/${modelKey}`;
  const memory = owner !== null && remembered?.owner === owner ? remembered : null;

  const [graph, setGraph] = useState<FlowGraph | null>(memory?.graph ?? null);
  const [expanded, setExpanded] = useState<string[]>(memory?.expanded ?? []);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    if (owner !== null) remembered = { owner, graph, expanded };
  }, [owner, graph, expanded]);

  const key = JSON.stringify([workspace, modelKey, readOnly, source, expanded]);
  // The source the last request was scheduled for: only a change of source waits for the debounce.
  const scheduledSource = useRef<string | null>(null);

  useEffect(() => {
    const edited = scheduledSource.current !== null && scheduledSource.current !== source;
    scheduledSource.current = source;
    const controller = new AbortController();
    const { signal } = controller;
    const send = () => {
      const params = { subflows: expanded };
      // FE-DATA-2 exception: generated fetchers called directly, for an abort signal per request (each edit cancels the last).
      const request = readOnly && modelKey !== "new"
        ? workflowGetFlowGraph(workspace, modelKey, params, { signal })
        : workflowGraphUnsavedFlow(workspace, { source }, params, { signal });
      request.then(
        (res) => {
          if (signal.aborted) return;
          if (res.status === 200) {
            setGraph(res.data);
            setAnswer({ key, kind: "ok" });
          } else {
            setAnswer({ key, kind: res.status === 400 ? "unparsable" : "error", message: res.data.error });
          }
        },
        (err: unknown) => {
          if (signal.aborted) return;
          const unparsable = err instanceof ApiError && err.status === 400;
          setAnswer({ key, kind: unparsable ? "unparsable" : "error", message: messageOf(err) });
        },
      );
    };
    // Opening the tab or expanding a Subflow asks at once; an edit waits for the next keystroke.
    const timer = edited ? setTimeout(send, TOPOLOGY_DEBOUNCE_MS) : undefined;
    if (!edited) send();
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
    // `key` changes with each of the others; they are listed for the linter.
  }, [key, workspace, modelKey, readOnly, source, expanded]);

  const onExpandSubflow = useCallback((uid: string) => {
    setExpanded((now) => (now.includes(uid) ? now : [...now, uid]));
  }, []);

  const onSelect = useCallback((uid: string) => {
    setSelected(uid);
    const taskId = (u: string) => {
      const n = graph?.nodes.find((x) => x.uid === u);
      return n && n.node === "task" ? n.task_id : n && n.node === "trigger" ? n.trigger_id : u;
    };
    // A uid inside an expansion is `{subflow}/{child}`: the child is in another flow's YAML.
    const slash = uid.indexOf("/");
    if (slash >= 0) {
      setNote(`${taskId(uid)} is a task of the Subflow ${taskId(uid.slice(0, slash))}'s flow. Open that flow to edit it.`);
      return;
    }
    const range = rangeOfPath(source, uid);
    if (!range) {
      setNote(`${taskId(uid)} is not in the YAML as it is now.`);
      return;
    }
    setNote(null);
    focusRange(range);
  }, [graph, source, focusRange]);

  const busy = answer?.key !== key;
  const stale = answer?.kind === "unparsable" && graph !== null;
  const error = answer && answer.kind !== "ok" && !stale ? answer.message : null;

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-2">
      <p role="status" className="text-sm text-muted-foreground empty:hidden">{note}</p>
      <div className="min-h-0 flex-1">
        <TopologyView
          graph={graph}
          selectedUid={selected}
          onSelect={onSelect}
          onExpandSubflow={onExpandSubflow}
          busy={busy}
          error={error}
          stale={stale}
          viewKey={`${workspace}/${modelKey}`}
        />
      </div>
    </div>
  );
}
