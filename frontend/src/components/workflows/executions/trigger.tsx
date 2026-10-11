import {
  CalendarClock,
  CircleHelp,
  CornerDownRight,
  History,
  Repeat,
  RotateCcw,
  Terminal,
  Webhook,
  Workflow,
  type LucideIcon,
} from "lucide-react";

/**
 * WHAT STARTED AN EXECUTION, as a word and a glyph (W7b spec §4.1).
 *
 * The engine stores Kestra's `ExecutionTrigger` (`{id, type, variables}`) in
 * `execution_trigger`, and these are the types it writes:
 * - `Schedule` (`scheduler/fire.rs` `SCHEDULE_TYPE`);
 * - `Webhook` (`trigger/webhook.rs`);
 * - `Flow`, a flow trigger (`flow_trigger/process.rs` `FLOW_TYPE`);
 * - `Subflow`, a child started by a `Subflow` task (`worker/subflow.rs`).
 *
 * No record means no trigger fired. That is an API start (the Run button is
 * one), unless the row says otherwise: a loop iteration and a
 * `CREATE_NEW_EXECUTION` retry carry `parent_execution_id` (the iteration also
 * a `loop_index`), and a manual replay `replayed_from_execution_id`.
 *
 * A type this table does not know is shown by its last segment, with the full
 * name in its title, rather than guessed at.
 */
export type TriggerKind = "api" | "schedule" | "webhook" | "flow" | "subflow" | "loop" | "retry" | "replay" | "other";

const BY_TYPE = new Map<string, { kind: TriggerKind; label: string }>([
  ["io.kestra.plugin.core.trigger.Schedule", { kind: "schedule", label: "Schedule" }],
  ["io.kestra.plugin.core.trigger.Webhook", { kind: "webhook", label: "Webhook" }],
  ["io.kestra.plugin.core.trigger.Flow", { kind: "flow", label: "Flow trigger" }],
  ["io.kestra.plugin.core.flow.Subflow", { kind: "subflow", label: "Subflow" }],
]);

const ICONS: Record<TriggerKind, LucideIcon> = {
  api: Terminal,
  schedule: CalendarClock,
  webhook: Webhook,
  flow: Workflow,
  subflow: CornerDownRight,
  loop: Repeat,
  retry: RotateCcw,
  replay: History,
  other: CircleHelp,
};

export interface TriggerSource {
  execution_trigger?: unknown;
  parent_execution_id?: string | null;
  loop_index?: number | null;
  replayed_from_execution_id?: string | null;
}

/** The trigger's `type`, when the record has one. */
function triggerType(row: TriggerSource): string | undefined {
  const t = row.execution_trigger;
  if (t && typeof t === "object" && typeof (t as { type?: unknown }).type === "string") return (t as { type: string }).type;
  return undefined;
}

export function triggerKind(row: TriggerSource): { kind: TriggerKind; label: string } {
  const type = triggerType(row);
  if (type) {
    const known = BY_TYPE.get(type);
    if (known) return known;
    return { kind: "other", label: type.slice(type.lastIndexOf(".") + 1) || type };
  }
  if (row.parent_execution_id) {
    return row.loop_index != null ? { kind: "loop", label: "Loop iteration" } : { kind: "retry", label: "Retry" };
  }
  if (row.replayed_from_execution_id) return { kind: "replay", label: "Replay" };
  return { kind: "api", label: "API" };
}

/** The trigger as a glyph and its word; the glyph is decoration, the word is the name (FE-A11Y-4). */
export function TriggerLabel({ row }: { row: TriggerSource }) {
  const { kind, label } = triggerKind(row);
  const Icon = ICONS[kind];
  return (
    <span className="inline-flex items-center gap-1.5" title={kind === "other" ? triggerType(row) : undefined}>
      <Icon aria-hidden="true" className="h-4 w-4 shrink-0 text-muted-foreground" />
      {label}
    </span>
  );
}
