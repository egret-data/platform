import type { FlowTriggersDto, ScheduleInfoDto, TriggerEntryDto } from "@/api/generated/model";

/**
 * Trigger entries as `GET …/flows/{id}/triggers` answers them (W7e spec §3.5),
 * one builder per kind. Every field a test does not set is the "on, firing"
 * value, so a test names only what makes its case.
 */
export function entry(over: Partial<TriggerEntryDto> = {}): TriggerEntryDto {
  return {
    trigger_id: "t",
    type: "io.kestra.plugin.core.trigger.Webhook",
    kind: "webhook",
    yaml_disabled: false,
    runtime_disabled: false,
    disabled_by: null,
    disabled_by_name: null,
    disabled_at: null,
    effective_disabled: false,
    can_toggle: true,
    toggle_refusal: null,
    duplicate_id: false,
    schedule: null,
    flow: null,
    webhook: null,
    last_execution: null,
    ...over,
  };
}

export function schedule(over: Partial<ScheduleInfoDto> = {}): ScheduleInfoDto {
  return {
    cron: "*/5 * * * *",
    with_seconds: false,
    timezone: "UTC",
    next_evaluation_date: "2099-01-01T00:05:00Z",
    evaluated_at: "2099-01-01T00:00:00Z",
    last_error: null,
    off_queue: false,
    missed_ticks: null,
    recover_missed_schedules: null,
    ...over,
  };
}

export const scheduleEntry = (over: Partial<TriggerEntryDto> = {}, sched: Partial<ScheduleInfoDto> = {}) =>
  entry({ trigger_id: "daily", type: "io.kestra.plugin.core.trigger.Schedule", kind: "schedule", schedule: schedule(sched), ...over });

export const webhookEntry = (over: Partial<TriggerEntryDto> = {}) =>
  entry({
    trigger_id: "hook",
    type: "io.kestra.plugin.core.trigger.Webhook",
    kind: "webhook",
    webhook: { url_path: "/workflows/v1/eng/executions/webhook/company.team/hello/s3cr3t-key", key_error: null, unsupported: null },
    ...over,
  });

export const flowEntry = (over: Partial<TriggerEntryDto> = {}) =>
  entry({
    trigger_id: "upstream",
    type: "io.kestra.plugin.core.trigger.Flow",
    kind: "flow",
    flow: { states: ["SUCCESS"], listening: true, not_listening_reason: null, depends_on: null },
    ...over,
  });

export const otherEntry = (over: Partial<TriggerEntryDto> = {}) =>
  entry({
    trigger_id: "poll",
    type: "io.kestra.plugin.fs.http.Trigger",
    kind: "other",
    can_toggle: false,
    toggle_refusal: "not_supported",
    ...over,
  });

export function flowTriggers(over: Partial<FlowTriggersDto> = {}): FlowTriggersDto {
  return {
    flow_disabled: false,
    flow_deleted: false,
    scheduler_enabled: true,
    worker_enabled: true,
    webhooks_enabled: true,
    triggers: [],
    ...over,
  };
}

/** A flow source declaring the four triggers above, in their order. */
export const TRIGGERS_SOURCE = `id: hello
namespace: company.team
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: hi
triggers:
  - id: daily
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "*/5 * * * *"
  - id: hook
    type: io.kestra.plugin.core.trigger.Webhook
    key: s3cr3t-key
  - id: upstream
    type: io.kestra.plugin.core.trigger.Flow
    conditions:
      - type: io.kestra.plugin.core.condition.ExecutionStatus
        in: [SUCCESS]
  - id: poll
    type: io.kestra.plugin.fs.http.Trigger
    uri: https://example.com
`;
