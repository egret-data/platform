import { parse } from "yaml";
import type { TaskRunDto } from "@/api/generated/model";

/** Why "Replay from this task" is disabled on a run beneath a `WorkingDirectory` (FE-ACTION-4). */
export const INSIDE_WORKING_DIRECTORY_REASON = "This run is inside a WorkingDirectory block. Replay from the block itself, or replay the whole execution.";

/**
 * Does `type` name a `WorkingDirectory`, by the server's spelling rule (`models::canonical_flow_type`):
 * the name after `io.kestra.plugin.core.flow.` or `core.`, in any case.
 */
function isWorkingDirectoryType(type: string): boolean {
  const name = type.startsWith("io.kestra.plugin.core.flow.")
    ? type.slice("io.kestra.plugin.core.flow.".length)
    : type.startsWith("core.")
      ? type.slice("core.".length)
      : null;
  return name?.toLowerCase() === "workingdirectory";
}

/**
 * The ids of every `WorkingDirectory` task in a flow source, at any depth. Task ids are unique in a
 * flow, so a run's `task_id` names its task. Empty when the source does not parse: the page then
 * offers the action and the server answers with its own sentence.
 */
export function workingDirectoryTaskIds(source: string): Set<string> {
  const ids = new Set<string>();
  let doc: unknown;
  try {
    doc = parse(source);
  } catch {
    return ids;
  }
  const walk = (node: unknown) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
    } else if (node !== null && typeof node === "object") {
      const o = node as Record<string, unknown>;
      if (typeof o.id === "string" && typeof o.type === "string" && isWorkingDirectoryType(o.type)) ids.add(o.id);
      Object.values(o).forEach(walk);
    }
  };
  walk(doc);
  return ids;
}

type RunLink = Pick<TaskRunDto, "run_ix" | "parent_ix" | "task_id">;

/**
 * Is `run` beneath a `WorkingDirectory` run (the server's `inside_working_directory` refusal, which
 * reads the run's ancestors on the frozen source's plan)? The block's own run is not inside it.
 */
export function insideWorkingDirectory(run: Pick<TaskRunDto, "parent_ix">, runs: readonly RunLink[], wd: ReadonlySet<string>): boolean {
  if (wd.size === 0) return false;
  const byIx = new Map(runs.map((r) => [r.run_ix, r]));
  const seen = new Set<number>();
  let parent = run.parent_ix ?? null;
  while (parent !== null && !seen.has(parent)) {
    seen.add(parent);
    const p = byIx.get(parent);
    if (!p) return false;
    if (wd.has(p.task_id)) return true;
    parent = p.parent_ix ?? null;
  }
  return false;
}
