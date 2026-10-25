import type {
  BackfillDto,
  CannotBackfill,
  FlowTriggersDto,
  MissedTicksDto,
  SetTriggerDisabledRequest,
  TriggerEntryDto,
  TriggerKindDto,
} from "@/api/generated/model";
import type { Enablement } from "@/components/workflows/draftState";
import { WORKER_OFF_TEXT } from "@/components/workflows/workerOff";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";

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
  if (entry.backfill) return backfillStatus(entry.backfill);
  if (entry.flow && !entry.flow.listening && entry.flow.not_listening_reason) {
    return `Not listening: ${entry.flow.not_listening_reason}`;
  }
  if (entry.kind === "other") return "Not run by egret";
  if (entry.webhook?.key_error) return "Key does not render";
  if (entry.webhook?.unsupported) return `Not served by egret: ${entry.webhook.unsupported}`;
  return "Active";
}

/**
 * Whether a trigger fires at all (KI-189): not off in its YAML or by the runtime switch, and its
 * flow neither disabled nor in the trash. A trigger that does not fire shows no next run.
 */
export function firesNext(
  entry: Pick<TriggerEntryDto, "yaml_disabled" | "runtime_disabled">,
  ctx: FlowContext,
): boolean {
  return !entry.yaml_disabled && !entry.runtime_disabled && !ctx.flowDisabled && !ctx.flowDeleted;
}

export const MANAGE_REASON = "You need workflow:manage";

/**
 * Why the bulk actions wait (KI-226): said once for all of them, not once per button. They are
 * shown only to a caller holding `workflow:manage`, the only one who can select.
 */
export const SELECT_TRIGGERS_HINT = "Select triggers to switch them or change their backfills";

/** The bulk actions' enablement: on with a selection, else waiting for one. */
export function bulkEnablement(selected: number): Enablement {
  return selected === 0 ? { ok: false, reason: SELECT_TRIGGERS_HINT } : { ok: true };
}

/** A backfill's progress, as the server computed it (Kestra's formula, `BackfillDto.progress`). */
export const progressText = (b: Pick<BackfillDto, "progress">): string => `${b.progress}%`;

/** "Backfilling (40%)", or "Backfill paused (40%)": the status while a backfill is on the trigger. */
export function backfillStatus(b: BackfillDto): string {
  return b.paused ? `Backfill paused (${progressText(b)})` : `Backfilling (${progressText(b)})`;
}

/** "3 of 10 ticks": what the strip and the drawer say of the window's progress. */
export const ticksText = (b: Pick<BackfillDto, "ticks_done" | "total_ticks">): string =>
  `${b.ticks_done} of ${b.total_ticks} ticks`;

/** The window's ends in the viewer's time, as the strip and the drawer show them. */
export const backfillWindowText = (b: Pick<BackfillDto, "start" | "end">): string =>
  `${fullTimestamp(b.start)} → ${fullTimestamp(b.end)}`;

/** Why the Backfill button is off, in words (FE-ACTION-4); `lastError` is the schedule's, for `off_queue`. */
export function backfillRefusalText(r: CannotBackfill, lastError?: string | null): string {
  switch (r) {
    case "not_schedule":
      return "Only schedules can be backfilled";
    case "forbidden":
      return MANAGE_REASON;
    case "disabled":
      return "Enable the trigger to backfill it";
    case "off_queue":
      return lastError ? `Off the queue: ${lastError}` : "Off the queue: fix the flow and save it";
    case "flow_deleted":
      return "The flow is in the trash";
    case "running":
      return "This trigger already has a backfill";
  }
}

/** A backfill whose trigger is switched off (YAML or runtime): frozen, it fires nothing until the trigger is on. */
export const backfillFrozen = (entry: Pick<TriggerEntryDto, "runtime_disabled" | "yaml_disabled">): boolean =>
  entry.runtime_disabled || entry.yaml_disabled;

/** Whether any listed trigger has a backfill that is running (not paused): the tab polls faster then (§4.1). */
export const anyBackfillRunning = (triggers: Pick<TriggerEntryDto, "backfill">[]): boolean =>
  triggers.some((t) => t.backfill != null && !t.backfill.paused);

/** The bulk backfill route's refusal, named for the user: `no_backfill` reads "had no backfill". */
export function backfillBulkMessage(code: string | null | undefined, message: string | null | undefined): string {
  if (code === "no_backfill") return "had no backfill";
  return message ?? code ?? "refused";
}

const REFUSALS: Record<string, string> = {
  yaml_disabled: "Disabled in the flow's YAML",
  not_supported: "Not run by egret",
  forbidden: MANAGE_REASON,
  flow_deleted: "The flow is in the trash",
};

/**
 * Whether a trigger's switch can be used, and why not (FE-ACTION-4). The
 * server's `toggle_refusal` is the authority; a viewer without
 * `workflow:manage` is refused here too, whatever the entry says. A switch in
 * flight is not "disabled" here: the table keeps it focusable and marks it
 * `aria-disabled` instead, so the keyboard focus is not dropped.
 */
export function switchEnablement(entry: TriggerEntryDto, ctx: { manage: boolean }): Enablement {
  if (!entry.can_toggle) {
    return { ok: false, reason: (entry.toggle_refusal && REFUSALS[entry.toggle_refusal]) || "This trigger cannot be switched" };
  }
  if (!ctx.manage) return { ok: false, reason: MANAGE_REASON };
  return { ok: true };
}

/**
 * Switching this trigger on must ask about missed runs (spec §4.3): a
 * switched-off `Schedule` on the queue whose next tick has passed. Only a
 * switched-off trigger: enabling one already on changes nothing on the server
 * (§3.3), so there is nothing to ask.
 *
 * "Passed" is the server's word first: `missed_ticks` is set exactly when the
 * server, at the database's `now()`, found the tick due, so a browser clock
 * behind the server's cannot hide a backlog (the enable would then carry
 * `recover_missed: false` and the server would skip it). `now`, the
 * browser's clock, only adds: a response read before the tick came due.
 */
export function needsEnableDialog(entry: TriggerEntryDto, now: number): boolean {
  const s = entry.schedule;
  if (entry.kind !== "schedule" || !s || !entry.runtime_disabled || s.off_queue || !s.next_evaluation_date) return false;
  if (s.missed_ticks) return true;
  // While a backfill holds the trigger, its date is the window's tick: the regular schedule's
  // position is the one the backfill saved (W7f §4.1), so that is what has passed or not.
  const regular = entry.backfill ? entry.backfill.previous_next_evaluation_date : s.next_evaluation_date;
  return Date.parse(regular) <= now;
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
    out.push({ id: "worker", text: WORKER_OFF_TEXT });
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
