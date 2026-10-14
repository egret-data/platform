import { describe, expect, it } from "vitest";
import {
  bannersFor,
  enableBody,
  kindLabel,
  missedLabel,
  needsEnableDialog,
  recoverDescription,
  shortType,
  statusText,
  switchEnablement,
  webhookNote,
  webhookPath,
} from "@/components/workflows/triggers/triggersModel";
import { flowEntry, flowTriggers, otherEntry, scheduleEntry, webhookEntry } from "./triggerFixtures";

const ON = { flowDisabled: false, flowDeleted: false };
const NOW = Date.parse("2026-10-15T12:00:00Z");

describe("statusText", () => {
  // One entry carrying every cause at once, then each cause removed in turn:
  // what is left says the next one, so the order is pinned, not just each sentence.
  const minutesAgo = new Date(Date.now() - 3 * 60_000).toISOString();
  const everything = scheduleEntry(
    { yaml_disabled: true, runtime_disabled: true, disabled_by_name: "Ana Lima", disabled_at: minutesAgo },
    { off_queue: true, next_evaluation_date: null, last_error: "the cron has no tick after now" },
  );

  it("says why a trigger does not fire, first match wins", () => {
    expect(statusText(everything, ON)).toBe("Disabled in YAML");
    const runtime = { ...everything, yaml_disabled: false };
    expect(statusText(runtime, ON)).toBe("Switched off by Ana Lima 3 minutes ago");
    expect(statusText({ ...runtime, runtime_disabled: false }, ON)).toBe("Off the queue: the cron has no tick after now");
  });

  it("names the switcher only when the server named them", () => {
    const t = scheduleEntry({ runtime_disabled: true, disabled_by_name: null, disabled_at: null });
    expect(statusText(t, ON)).toBe("Switched off");
  });

  it("says the flow is in the trash, or disabled, before the trigger's own state (D18)", () => {
    const off = scheduleEntry({}, { off_queue: true, last_error: "no tick" });
    expect(statusText(off, { flowDisabled: true, flowDeleted: true })).toBe("The flow is in the trash");
    expect(statusText(off, { flowDisabled: true, flowDeleted: false })).toBe("On, but the flow is disabled, so it does not fire");
    // A switched-off trigger in a disabled flow says it is switched off: that is what the switch shows.
    expect(statusText(scheduleEntry({ runtime_disabled: true }), { flowDisabled: true, flowDeleted: false })).toBe("Switched off");
  });

  it("covers each kind's own reason, and Active otherwise", () => {
    expect(statusText(flowEntry({ flow: { states: [], listening: false, not_listening_reason: "the namespace is not a string" } }), ON))
      .toBe("Not listening: the namespace is not a string");
    expect(statusText(otherEntry(), ON)).toBe("Not run by egret");
    expect(statusText(webhookEntry({ webhook: { url_path: null, key_error: "the key renders to nothing", unsupported: null } }), ON))
      .toBe("Key does not render");
    expect(statusText(webhookEntry({ webhook: { url_path: "/w/k", key_error: null, unsupported: "responseContentType" } }), ON))
      .toBe("Not served by egret: responseContentType");
    expect(statusText(webhookEntry(), ON)).toBe("Active");
    expect(statusText(scheduleEntry(), ON)).toBe("Active");
    expect(statusText(flowEntry(), ON)).toBe("Active");
  });

  it("does not report an off-queue schedule without its reason as Active", () => {
    expect(statusText(scheduleEntry({}, { off_queue: true, last_error: null }), ON)).toBe("Off the queue");
  });
});

describe("switchEnablement", () => {
  it("gives each refusal its sentence", () => {
    const manage = { manage: true };
    expect(switchEnablement(scheduleEntry({ can_toggle: false, toggle_refusal: "yaml_disabled" }), manage))
      .toEqual({ ok: false, reason: "Disabled in the flow's YAML" });
    expect(switchEnablement(otherEntry(), manage)).toEqual({ ok: false, reason: "Not run by egret" });
    expect(switchEnablement(scheduleEntry({ can_toggle: false, toggle_refusal: "forbidden" }), manage))
      .toEqual({ ok: false, reason: "You need workflow:manage" });
    expect(switchEnablement(scheduleEntry({ can_toggle: false, toggle_refusal: "flow_deleted" }), manage))
      .toEqual({ ok: false, reason: "The flow is in the trash" });
  });

  it("is on for a togglable trigger, and off for a viewer whatever the server says", () => {
    expect(switchEnablement(scheduleEntry(), { manage: true })).toEqual({ ok: true });
    expect(switchEnablement(scheduleEntry(), { manage: false })).toEqual({ ok: false, reason: "You need workflow:manage" });
  });

});

describe("needsEnableDialog", () => {
  const off = { runtime_disabled: true, effective_disabled: true };
  it("asks for a switched-off schedule whose next tick has passed", () => {
    expect(needsEnableDialog(scheduleEntry(off, { next_evaluation_date: "2026-10-15T11:55:00Z" }), NOW)).toBe(true);
    // A tick exactly now has passed too: the scheduler's claim is `<= now()`.
    expect(needsEnableDialog(scheduleEntry(off, { next_evaluation_date: "2026-10-15T12:00:00Z" }), NOW)).toBe(true);
  });
  it("asks when the server counted missed ticks, though the browser's clock reads the tick as ahead", () => {
    // The server computes missed_ticks against the database's now(); a client
    // clock behind it must not hide the backlog.
    const skewed = scheduleEntry(off, { next_evaluation_date: "2026-10-15T12:05:00Z", missed_ticks: { count: 2, more: false } });
    expect(needsEnableDialog(skewed, NOW)).toBe(true);
  });
  it("does not ask for a future tick, an off-queue row, an already-on schedule or another kind", () => {
    expect(needsEnableDialog(scheduleEntry(off, { next_evaluation_date: "2026-10-15T12:05:00Z" }), NOW)).toBe(false);
    expect(needsEnableDialog(scheduleEntry(off, { off_queue: true, next_evaluation_date: "2026-10-15T11:55:00Z" }), NOW)).toBe(false);
    expect(needsEnableDialog(scheduleEntry(off, { next_evaluation_date: null }), NOW)).toBe(false);
    expect(needsEnableDialog(scheduleEntry({}, { next_evaluation_date: "2026-10-15T11:55:00Z" }), NOW)).toBe(false);
    expect(needsEnableDialog(webhookEntry(off), NOW)).toBe(false);
  });
});

describe("the enable dialog's words and body", () => {
  it("describes recovery per recoverMissedSchedules", () => {
    expect(recoverDescription(null)).toBe("runs every missed tick, one per scheduler pass");
    expect(recoverDescription(undefined)).toBe("runs every missed tick, one per scheduler pass");
    expect(recoverDescription("ALL")).toBe("runs every missed tick, one per scheduler pass");
    expect(recoverDescription("LAST")).toBe("runs only the latest missed tick");
    expect(recoverDescription("NONE")).toBe("skips them: this trigger's recoverMissedSchedules is NONE");
    expect(recoverDescription("SOME")).toBe("is refused: this trigger's recoverMissedSchedules is SOME, which is not ALL, LAST or NONE");
  });

  it("counts missed ticks up to 1000, then 1000+", () => {
    expect(missedLabel({ count: 3, more: false })).toBe("3");
    expect(missedLabel({ count: 1000, more: true })).toBe("1000+");
    expect(missedLabel(null)).toBe("0");
  });

  it("sends recover_missed per choice", () => {
    expect(enableBody("skip")).toEqual({ disabled: false, recover_missed: false });
    expect(enableBody("recover")).toEqual({ disabled: false, recover_missed: true });
  });
});

describe("bannersFor", () => {
  it("raises each banner only for what it concerns", () => {
    const all = flowTriggers({
      scheduler_enabled: false,
      worker_enabled: false,
      webhooks_enabled: false,
      flow_disabled: true,
      triggers: [scheduleEntry(), webhookEntry()],
    });
    expect(bannersFor(all).map((b) => b.id)).toEqual(["scheduler", "worker", "webhooks", "flow"]);
    // The scheduler banner needs a Schedule or Flow trigger; the webhook one a Webhook.
    expect(bannersFor({ ...all, triggers: [otherEntry()] }).map((b) => b.id)).toEqual(["worker", "flow"]);
    expect(bannersFor({ ...all, triggers: [flowEntry()] }).map((b) => b.id)).toEqual(["scheduler", "worker", "flow"]);
    expect(bannersFor(flowTriggers({ triggers: [scheduleEntry(), webhookEntry()] }))).toEqual([]);
  });

  it("reads a global page's rows as well", () => {
    expect(bannersFor({ scheduler_enabled: true, worker_enabled: true, webhooks_enabled: false, rows: [webhookEntry()] }).map((b) => b.id))
      .toEqual(["webhooks"]);
  });
});

describe("the webhook path (D17)", () => {
  it("keeps a single leading slash, never an origin", () => {
    expect(webhookPath("/workflows/v1/eng/x")).toBe("/workflows/v1/eng/x");
    expect(webhookPath("//workflows/v1/eng/x")).toBe("/workflows/v1/eng/x");
    expect(webhookPath("workflows/v1/eng/x")).toBe("/workflows/v1/eng/x");
    expect(webhookPath(null)).toBeNull();
  });

  it("notes where the path is served, and when webhooks are off", () => {
    expect(webhookNote(true)).toBe("This path is served by the egret backend, at whatever host your operator exposes for webhooks.");
    expect(webhookNote(false)).toBe(
      "This path is served by the egret backend, at whatever host your operator exposes for webhooks. Webhooks are off on this server.",
    );
  });
});

describe("names", () => {
  it("labels kinds and shortens types", () => {
    expect(["schedule", "webhook", "flow", "other"].map((k) => kindLabel(k as never))).toEqual(["Schedule", "Webhook", "Flow", "Other"]);
    expect(shortType("io.kestra.plugin.core.trigger.Schedule")).toBe("Schedule");
    expect(shortType("Schedule")).toBe("Schedule");
  });
});
