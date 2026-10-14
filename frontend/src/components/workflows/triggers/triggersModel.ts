import type {
  FlowTriggersDto,
  MissedTicksDto,
  SetTriggerDisabledRequest,
  TriggerEntryDto,
  TriggerKindDto,
} from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { rangeOfPath } from "@/components/workflows/editor/yamlPath";
import { timeAgo } from "@/lib/relativeTime";

/**
 * The words and decisions of the Triggers views (W7e spec §4), pure so the flow
 * tab and the global page (Task 7) say the same things. Every sentence a
 * trigger's state produces is written here.
 */

/** A trigger as the views take it: the per-flow entry plus the flow's uuid the switch routes need. */
export type TriggerRow = TriggerEntryDto & { flow_uuid: string };

/** The flow-level facts a trigger's status depends on (the per-flow route's top flags). */
export type FlowContext = { flowDisabled: boolean; flowDeleted: boolean };

const KIND_LABELS: Record<TriggerKindDto, string> = {
  schedule: "Schedule",
  webhook: "Webhook",
  flow: "Flow",
  other: "Other",
};

export const kindLabel = (kind: TriggerKindDto): string => KIND_LABELS[kind] ?? kind;

/** The last dotted segment of a plugin type: `io.kestra.plugin.core.trigger.Schedule` is `Schedule`. */
export const shortType = (type: string): string => type.slice(type.lastIndexOf(".") + 1);

/** The status of a trigger that is switched on in a flow that fires nothing (D18). */
export const FLOW_DISABLED_STATUS = "On, but the flow is disabled, so it does not fire";

/**
 * Why a trigger does not fire, in words (spec §4.1, FE-A11Y-4); "Active" when
 * nothing stops it. The first cause wins: the YAML, then the runtime switch,
 * then the flow (in the trash, or disabled: D18), then the kind's own state.
 */
export function statusText(entry: TriggerEntryDto, ctx: FlowContext): string {
  if (entry.yaml_disabled) return "Disabled in YAML";
  if (entry.runtime_disabled) {
    const by = entry.disabled_by_name ? ` by ${entry.disabled_by_name}` : "";
    const when = entry.disabled_at ? ` ${timeAgo(entry.disabled_at)}` : "";
    return `Switched off${by}${when}`;
  }
  if (ctx.flowDeleted) return "The flow is in the trash";
  if (ctx.flowDisabled) return FLOW_DISABLED_STATUS;
  if (entry.schedule?.off_queue) {
    return entry.schedule.last_error ? `Off the queue: ${entry.schedule.last_error}` : "Off the queue";
  }
  if (entry.flow && !entry.flow.listening && entry.flow.not_listening_reason) {
    return `Not listening: ${entry.flow.not_listening_reason}`;
  }
  if (entry.kind === "other") return "Not run by egret";
  if (entry.webhook?.key_error) return "Key does not render";
  return "Active";
}

export const MANAGE_REASON = "You need workflow:manage";

const REFUSALS: Record<string, string> = {
  yaml_disabled: "Disabled in the flow's YAML",
  not_supported: "Not run by egret",
  forbidden: MANAGE_REASON,
  flow_deleted: "The flow is in the trash",
};

/**
 * Whether a trigger's switch can be used, and why not (FE-ACTION-4). The
 * server's `toggle_refusal` is the authority; a viewer without
 * `workflow:manage` is refused here too, whatever the entry says.
 */
export function switchEnablement(entry: TriggerEntryDto, ctx: { manage: boolean; busy?: boolean }): Enablement {
  if (!entry.can_toggle) {
    return { ok: false, reason: (entry.toggle_refusal && REFUSALS[entry.toggle_refusal]) || "This trigger cannot be switched" };
  }
  if (!ctx.manage) return { ok: false, reason: MANAGE_REASON };
  if (ctx.busy) return { ok: false, reason: "Switching…" };
  return { ok: true };
}

/**
 * Switching this trigger on must ask about missed runs (spec §4.3): a
 * switched-off `Schedule` on the queue whose next tick has passed. Only a
 * switched-off trigger: enabling one already on changes nothing on the server
 * (§3.3), so there is nothing to ask.
 */
export function needsEnableDialog(entry: TriggerEntryDto, now: number): boolean {
  const s = entry.schedule;
  if (entry.kind !== "schedule" || !s || !entry.runtime_disabled || s.off_queue || !s.next_evaluation_date) return false;
  return Date.parse(s.next_evaluation_date) <= now;
}

/** What "Recover missed runs" does for a trigger, per its `recoverMissedSchedules` (absent = `ALL`). */
export function recoverDescription(raw: string | null | undefined): string {
  if (raw === null || raw === undefined || raw === "ALL") return "runs every missed tick, one per scheduler pass";
  if (raw === "LAST") return "runs only the latest missed tick";
  if (raw === "NONE") return "skips them: this trigger's recoverMissedSchedules is NONE";
  return `is refused: this trigger's recoverMissedSchedules is ${raw}, which is not ALL, LAST or NONE`;
}

/** The server counts missed ticks up to 1,000 and says when there are more. */
export function missedLabel(missed: MissedTicksDto | null | undefined): string {
  if (!missed) return "0";
  return missed.more ? `${missed.count}+` : String(missed.count);
}

export type EnableChoice = "skip" | "recover";

export function enableBody(choice: EnableChoice): SetTriggerDisabledRequest {
  return { disabled: false, recover_missed: choice === "recover" };
}

export type Banner = { id: "scheduler" | "worker" | "webhooks" | "flow"; text: string };

type BannerPage = Pick<FlowTriggersDto, "scheduler_enabled" | "worker_enabled" | "webhooks_enabled"> & {
  flow_disabled?: boolean;
} & ({ triggers: Pick<TriggerEntryDto, "kind">[] } | { rows: Pick<TriggerEntryDto, "kind">[] });

/**
 * The banners above a trigger list, each saying what to do (spec §4.1). None
 * for a draft: a draft fires.
 */
export function bannersFor(page: BannerPage): Banner[] {
  const kinds = new Set(("triggers" in page ? page.triggers : page.rows).map((t) => t.kind));
  const out: Banner[] = [];
  if (!page.scheduler_enabled && (kinds.has("schedule") || kinds.has("flow"))) {
    out.push({
      id: "scheduler",
      text: "The scheduler is off on this server, so schedules and flow triggers do not fire. An administrator sets workflow.schedulerEnabled.",
    });
  }
  if (!page.worker_enabled) {
    out.push({ id: "worker", text: "The worker is off, so triggered executions are created but do not run." });
  }
  if (!page.webhooks_enabled && kinds.has("webhook")) {
    out.push({ id: "webhooks", text: "Webhooks are off on this server; their URLs answer 404." });
  }
  if (page.flow_disabled) out.push({ id: "flow", text: "This flow is disabled, so none of its triggers fire." });
  return out;
}

/**
 * The webhook's path as the server rendered it (D17): a path, never a URL.
 * egret does not know the host an operator exposes webhooks on, and nothing at
 * the browser's origin routes `/workflows/` to the backend, so no origin is
 * prefixed. Normalised to one leading slash.
 */
export function webhookPath(urlPath: string | null | undefined): string | null {
  if (!urlPath) return null;
  return `/${urlPath.replace(/^\/+/, "")}`;
}

export function webhookNote(webhooksEnabled: boolean): string {
  const note = "This path is served by the egret backend, at whatever host your operator exposes for webhooks.";
  return webhooksEnabled ? note : `${note} Webhooks are off on this server.`;
}

/**
 * The YAML block of one trigger, cut from `source` and dedented, or null when
 * `source` declares no trigger with that id. `rangeOfPath` finds where the
 * trigger's mapping starts; the block runs until the first non-blank line
 * indented less than that mapping (the next trigger's `- ` or the next
 * top-level key).
 */
export function triggerBlock(source: string, triggerId: string): string | null {
  const range = rangeOfPath(source, `triggers.${triggerId}`);
  if (!range) return null;
  const lines = source.split("\n");
  const indent = range.startCol - 1;
  const block = [lines[range.startLine - 1].slice(indent)];
  for (let i = range.startLine; i < lines.length; i++) {
    const line = lines[i];
    if (line.trim() !== "" && line.length - line.trimStart().length < indent) break;
    block.push(line.slice(indent));
  }
  while (block.length > 1 && block[block.length - 1].trim() === "") block.pop();
  return block.join("\n");
}
