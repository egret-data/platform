import { memo, useEffect, useRef, useState } from "react";
import {
  useSchedule,
  useScheduleMutations,
  type ScheduleType,
} from "@/api/alerts";
import { useDatabases } from "@/api/databases";
import { useDashboards } from "@/api/dashboards";
import { useCharts } from "@/api/charts";
import { useWorkspaceMembers } from "@/api/members";
import { useDataApiListEndpoints } from "@/api/generated/data-api/data-api";
import { DataapiMetric } from "@/api/generated/model";
import {
  DEFAULT_SCHEDULE,
  WEEKDAYS,
  fromCron,
  localTimezone,
  timezones,
  toCron,
  type StructuredSchedule,
} from "@/lib/cronSchedule";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertCircle } from "lucide-react";
import { toast } from "sonner";

/** The form's id, so the page header's Save button can submit it from outside. */
export const FORM_ID = "schedule-editor";

/** The sections the editor is split across, in nav order. */
export type SectionKey = "general" | "what" | "schedule" | "notification";

// Keyed on the schedule's own `type`, not a bare `isAlert` boolean: a
// `dataapi_alert` is not an `alert` (`type === "alert"` is false for it,
// same reason `model.rs`'s `to_new` keeps `is_engine_alert`/`is_dataapi_alert`
// as two separate flags), but it DOES have a condition, so it reads
// "Condition" too — the boolean version of this label read "Contents" for a
// dataapi_alert, borrowing the report tab's wording for a schedule that has
// no report contents at all (Task 6, fix round 1).
export const SECTIONS: { key: SectionKey; label: (type: ScheduleType) => string }[] = [
  { key: "general", label: () => "General" },
  { key: "what", label: (t) => (t === "report" ? "Contents" : "Condition") },
  { key: "schedule", label: () => "Schedule" },
  { key: "notification", label: () => "Notification" },
];
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
import { TagCombobox } from "@/components/ui/tag-combobox";
import { DestinationList } from "./DestinationList";
import { toDestinations, toRecipients, type Destination } from "./destinations";
import { SUBJECT_TOKENS, renderSubject } from "@/lib/subjectTemplate";

/** The comparison operators a condition offers, in the order the picker lists them. */
const OPERATORS = ["<", "<=", ">", ">=", "==", "!="];

/** The Endpoint picker's item for "no endpoint": a select item cannot carry `""`,
 *  which is what an unscoped watch saves, so this stands in for it on screen only. */
const WHOLE_WORKSPACE = "whole-workspace";

/**
 * What each Data API metric actually measures, including the empty-window
 * behaviour — copied from `dataapi_metric.rs`'s own doc comments, not
 * re-derived, for the same reason that module copies `activity.rs` rather
 * than re-deriving it: a number that disagrees with the source it describes
 * is a support ticket.
 *
 * The asymmetry (`dataapi_metric.rs`'s "empty-window asymmetry" doc): a
 * count of zero rows IS a measurement, so `call_count`/`error_count`/
 * `error_code_count` are a real `0`. A rate or a percentile over zero rows
 * is undefined, not zero, so `error_rate`/`p50`/`p95` report no value at
 * all and the run is recorded `noop` rather than firing on a false "0% / 0ms"
 * that would read as a healthy, silent endpoint.
 */
const DATAAPI_METRIC_HELP: Record<string, string> = {
  [DataapiMetric.error_rate]:
    "Share of calls in the window with a status code of 400 or higher (real traffic only — Try-it trials are excluded). An empty window has no denominator, so the run records noop instead of a false 0%.",
  [DataapiMetric.error_count]:
    "Number of calls in the window with a status code of 400 or higher. An empty window is a real 0 — it does not by itself tell you the endpoint was quiet rather than healthy; error rate is what answers that.",
  [DataapiMetric.call_count]:
    "Total number of calls in the window, real traffic only — Try-it trials are excluded. An empty window is a real 0, so this can fire on \"too few calls\" as well as too many.",
  [DataapiMetric.p50_duration_ms]:
    "Median response time, in milliseconds, of successful calls only (status 200–299) — a burst of failed requests cannot drag this down. An empty window (no successful calls) has no percentile to report, so the run records noop instead of firing.",
  [DataapiMetric.p95_duration_ms]:
    "95th-percentile response time, in milliseconds, of successful calls only (status 200–299) — a burst of failed requests cannot drag this down. An empty window (no successful calls) has no percentile to report, so the run records noop instead of firing.",
  [DataapiMetric.error_code_count]:
    "Number of calls in the window whose error code matches the value entered below, exactly. An empty window is a real 0.",
};

/**
 * The create/edit form for one schedule.
 *
 * Sections follow Superset's — General · Condition/Contents · Schedule ·
 * Notification · Advanced — because they map to the model and users arriving
 * from Superset recognise them.
 *
 * Validation is deliberately NOT duplicated here beyond `required`: the server
 * owns the rules (they are also CHECK constraints), and a second copy in the
 * browser is a second thing to drift. What this component must do well is SHOW
 * the server's message against the form, which is what `error` below is for.
 */
/**
 * The Timezone picker, memoised: it lists every IANA zone (about 400 items),
 * and a `ui/select` renders its items on every render even while closed, so
 * without `memo` each keystroke anywhere in the editor re-rendered all of them.
 */
const TimezoneSelect = memo(function TimezoneSelect({
  id,
  value,
  onChange,
}: {
  id: string;
  value: string;
  onChange: (zone: string) => void;
}) {
  const zones = timezones();
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger id={id}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {/* A stored zone the browser does not list (an older egret, or a
            newer IANA name) must stay selectable, or opening the editor
            would silently move the schedule to another timezone. */}
        {!zones.includes(value) && <SelectItem value={value}>{value}</SelectItem>}
        {zones.map((z) => (
          <SelectItem key={z} value={z}>{z}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
});

export function ScheduleEditor({
  ws,
  type,
  scheduleId,
  prefill,
  section,
  onSectionChange,
  onDirtyChange,
  onBusyChange,
  onDone,
}: {
  ws: string;
  type: ScheduleType;
  scheduleId: string | null;
  /** A target chosen elsewhere — the Subscribe item on a dashboard or chart. */
  prefill?: { kind: "chart" | "dashboard"; object: string };
  /** Which section to show. The form keeps ALL its state either way — only the
   *  rendering is narrowed, so switching never loses a typed value. */
  section: SectionKey;
  /** Used to jump to the section holding a field the user must fix. */
  onSectionChange?: (s: SectionKey) => void;
  /** Lets the hosting PAGE arm its unsaved-changes guard. A dialog did not need
   *  one; a route can be navigated away from mid-edit. */
  onDirtyChange?: (dirty: boolean) => void;
  /** The submit is in flight — the page's Save button disables on this. */
  onBusyChange?: (busy: boolean) => void;
  onDone: () => void;
}) {
  const { data: existing } = useSchedule(ws, scheduleId);
  const { create, update } = useScheduleMutations(ws);
  const { data: databases } = useDatabases(ws);
  const { data: dashboards } = useDashboards(ws);
  const { data: charts } = useCharts(ws);
  // Untrashed only (the default when `params` is omitted) — the same list
  // Activity's own endpoint filter uses. A dataapi_alert already pointed at a
  // NOW-trashed endpoint is handled below, the same way the Schedule
  // section's timezone picker keeps a stored-but-unlisted zone selectable.
  const endpointsQuery = useDataApiListEndpoints(ws);
  const endpoints = endpointsQuery.data?.status === 200 ? endpointsQuery.data.data : [];
  // Owners are USERS: an alert runs the query with an owner's permissions, and
  // a group has no identity to run as.
  const { data: members } = useWorkspaceMembers(ws);
  const userMembers = (members ?? []).filter((m) => m.principal_type === "user");

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [crontab, setCrontab] = useState("0 9 * * *");
  const [timezone, setTimezone] = useState(localTimezone);
  /**
   * The picker's own state, and whether we are using it at all.
   *
   * `rawCron` is not a power-user preference toggle — it is what an expression
   * outside the picker's vocabulary FORCES. Editing a `0 9 * * 1-5` schedule
   * through dropdowns would silently rewrite when the mail arrives, so the
   * editor drops to the raw field and says why.
   */
  const [sched, setSched] = useState<StructuredSchedule>(DEFAULT_SCHEDULE);
  const [rawCron, setRawCron] = useState(false);
  const [message, setMessage] = useState("");
  const [skipIfEmpty, setSkipIfEmpty] = useState(false);
  const [active, setActive] = useState(true);
  const [sql, setSql] = useState("");
  const [databaseId, setDatabaseId] = useState("");
  const [validatorType, setValidatorType] = useState<"not_null" | "operator">("not_null");
  const [op, setOp] = useState(">");
  const [threshold, setThreshold] = useState("0");
  /** Data API alerts only. `""` means "whole workspace" — see `buildBody`. */
  const [dataapiEndpointId, setDataapiEndpointId] = useState("");
  const [dataapiMetric, setDataapiMetric] = useState<string>(DataapiMetric.error_rate);
  const [dataapiErrorCode, setDataapiErrorCode] = useState("");
  const [dataapiWindowS, setDataapiWindowS] = useState("300");
  const [targetKind, setTargetKind] = useState<"dashboard" | "chart">(
    prefill?.kind ?? "dashboard",
  );
  const [targetId, setTargetId] = useState(prefill?.object ?? "");
  /** Reports only: additional objects delivered in the same message. */
  const [extraTargets, setExtraTargets] = useState<string[]>([]);
  const [reportFormat, setReportFormat] = useState("png");
  /**
   * Every destination this schedule delivers to, in order.
   *
   * One list rather than an email field + a slack field + a webhook textarea:
   * the server has always stored `recipients` as a list of `{kind, config}`
   * and delivered to all of them, so the flat form was the only thing making
   * "two Slack channels" impossible. See `./destinations.ts`.
   */
  const [destinations, setDestinations] = useState<Destination[]>([]);
  const [emailSubject, setEmailSubject] = useState("");
  const [messageFooter, setMessageFooter] = useState("");
  const [owners, setOwners] = useState<string[]>([]);
  // `grace_period_s`'s server default (migration 0110) is 14400s — four hours
  // — inherited from Superset, where every alert is a business one ("orders
  // dropped"). A dataapi_alert watches operational signals (error rate,
  // latency) where four quiet hours after a first page is too long; 900s (15
  // minutes) matches a typical on-call check-in cadence without re-paging on
  // every tick of a schedule that itself might run every few minutes. Applied
  // client-side only — the column's own default is unchanged, so a row
  // created any other way still gets 14400.
  const [gracePeriod, setGracePeriod] = useState(
    type === "dataapi_alert" ? "900" : "14400",
  );
  const [workingTimeout, setWorkingTimeout] = useState("3600");
  const [logRetention, setLogRetention] = useState("90");
  const [customWidth, setCustomWidth] = useState("");
  const [customHeight, setCustomHeight] = useState("");
  /** Alerts only: attach a rendering of this chart/dashboard when it fires. */
  const [attachTarget, setAttachTarget] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Populate from the loaded schedule when editing. Keyed on the row's id so
  // reopening the editor on a different schedule refills rather than keeping
  // the previous one's values.
  useEffect(() => {
    if (!existing) return;
    setName(existing.name);
    setDescription(existing.description ?? "");
    setCrontab(existing.crontab);
    setTimezone(existing.timezone);
    const parsed = fromCron(existing.crontab);
    setSched(parsed ?? DEFAULT_SCHEDULE);
    setRawCron(parsed === null);
    setMessage(existing.message ?? "");
    setMessageFooter(existing.message_footer ?? "");
    setSkipIfEmpty(existing.skip_if_empty ?? false);
    setActive(existing.active);
    setSql(existing.sql ?? "");
    setDatabaseId(existing.database_id ?? "");
    setValidatorType(existing.validator_type ?? "not_null");
    const cfg = (existing.validator_config ?? {}) as Record<string, unknown>;
    if (typeof cfg.op === "string") setOp(cfg.op);
    if (typeof cfg.threshold === "number") setThreshold(String(cfg.threshold));
    setDataapiEndpointId(existing.dataapi_endpoint_id ?? "");
    setDataapiMetric(existing.dataapi_metric ?? DataapiMetric.error_rate);
    setDataapiErrorCode(existing.dataapi_error_code ?? "");
    setDataapiWindowS(
      existing.dataapi_window_s == null ? "300" : String(existing.dataapi_window_s),
    );
    if (existing.chart_id) {
      setTargetKind("chart");
      setTargetId(existing.chart_id);
    } else if (existing.dashboard_id) {
      setTargetKind("dashboard");
      setTargetId(existing.dashboard_id);
    }
    setReportFormat(existing.report_format ?? "png");
    // The first stored target is the primary one shown above; the rest are
    // the extras. A pre-0115 report has no target rows at all and falls back
    // to the chart_id/dashboard_id the columns still hold.
    const stored = existing.targets ?? [];
    if (stored.length > 0) {
      setTargetKind(stored[0].kind === "chart" ? "chart" : "dashboard");
      setTargetId(stored[0].object_id);
      setExtraTargets(stored.slice(1).map((t) => t.object_id));
    }
    setEmailSubject(existing.email_subject ?? "");
    setOwners(existing.owners.map((o) => o.user_id));
    setGracePeriod(String(existing.grace_period_s ?? 14400));
    setWorkingTimeout(String(existing.working_timeout_s ?? 3600));
    setLogRetention(String(existing.log_retention_days ?? 90));
    setCustomWidth(existing.custom_width == null ? "" : String(existing.custom_width));
    setCustomHeight(existing.custom_height == null ? "" : String(existing.custom_height));
    setAttachTarget(Boolean(existing.chart_id || existing.dashboard_id));
    setDestinations(toDestinations(existing.recipients));
  }, [existing]);

  /**
   * Bring a rejected save into view.
   *
   * The banner renders at the top of a form tall enough to scroll, and Save
   * sits at the bottom — so without this the server's message ("webhook url
   * must be http(s)") is off-screen and pressing Save looks like it did
   * nothing at all. Applies to every validation error, not only webhooks.
   */
  /**
   * Put a placeholder where the caret is, not at the end.
   *
   * Appending looked right until it was clicked: "Weekly overview" + the
   * button produced `Weekly overview{{date}}`, which SENDS as
   * "Weekly overview2026-08-13" with no separator. And a subject like
   * "Orders — {{date}} — EU" is an ordinary thing to want, which an
   * append-only button cannot write at all.
   */
  const subjectRef = useRef<HTMLInputElement>(null);
  const insertToken = (token: string) => {
    const el = subjectRef.current;
    const text = `{{${token}}}`;
    const at = el?.selectionStart ?? emailSubject.length;
    const to = el?.selectionEnd ?? emailSubject.length;
    const next = emailSubject.slice(0, at) + text + emailSubject.slice(to);
    setEmailSubject(next);
    // Focus and caret are restored after the value lands, so typing continues
    // where the placeholder ended rather than jumping to the start.
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(at + text.length, at + text.length);
    });
  };

  const errorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (error) errorRef.current?.scrollIntoView({ block: "nearest" });
  }, [error]);

  const isAlert = type === "alert";
  const isDataapiAlert = type === "dataapi_alert";
  /** A stored owner is a user id; a chip has to show the person. An id with no
   *  matching member is shown as itself rather than hidden — a former member
   *  still owns the schedule, and silently dropping them from the field would
   *  delete them on the next save. */
  const memberName = (id: string) =>
    userMembers.find((m) => m.principal_id === id)?.principal_name ?? id;
  const defaultTarget =
    targetId || (targetKind === "dashboard" ? dashboards?.[0]?.id : charts?.[0]?.id) || "";

  function buildBody() {
    // The update REPLACES the recipient set, so a destination missing from
    // this array is a destination deleted. Credentials follow the three-way
    // rule — see `toRecipients`.
    const recipients = toRecipients(destinations);

    // An empty number field means "use the default", not zero — the server
    // would reject 0 for the timers and silently disable retention.
    const num = (v: string, fallback: number) => {
      const n = Number(v.trim());
      return v.trim() === "" || !Number.isFinite(n) ? fallback : n;
    };

    const shared = {
      name,
      description,
      active,
      crontab: rawCron ? crontab : toCron(sched),
      timezone,
      recipients,
      owners,
      email_subject: emailSubject.trim() || null,
      message: message.trim() || null,
      message_footer: messageFooter.trim() || null,
      skip_if_empty: skipIfEmpty,
      // See the `gracePeriod` state's own comment: a dataapi_alert's default
      // is shorter than the server's 14400s. Only the DEFAULT differs — a
      // value the user typed is sent exactly as typed either way.
      grace_period_s: num(gracePeriod, isDataapiAlert ? 900 : 14400),
      working_timeout_s: num(workingTimeout, 3600),
      log_retention_days: num(logRetention, 90),
      custom_width: customWidth.trim() ? num(customWidth, 0) : null,
      custom_height: customHeight.trim() ? num(customHeight, 0) : null,
    };

    if (isDataapiAlert) {
      return {
        ...shared,
        type: "dataapi_alert" as const,
        // ALWAYS sent, including an explicit `null` for "whole workspace" —
        // never left out. `PUT` is a full replace, and an omitted field and a
        // deliberate workspace-wide choice are otherwise the SAME request: the
        // server stores a missing `dataapi_endpoint_id` as `null`, which this
        // system defines as "watch every endpoint", and it cannot tell a
        // client that forgot the field from one that chose that on purpose
        // (Task 5 review). A watchdog test pins that this key is always
        // present in the body, with either a real id or `null`.
        dataapi_endpoint_id: dataapiEndpointId || null,
        dataapi_metric: dataapiMetric,
        // Only meaningful for error_code_count (schedule_dataapi_error_code
        // _needs_metric / _metric_needs_error_code, migration 0128) — sent as
        // `null`, not an empty string, for every other metric so a stale value
        // left over from switching away can never reach the server.
        dataapi_error_code:
          dataapiMetric === DataapiMetric.error_code_count
            ? dataapiErrorCode.trim() || null
            : null,
        dataapi_window_s: num(dataapiWindowS, 300),
        // schedule_dataapi_needs_source pins this to 'operator' — there is no
        // not_null choice for a dataapi_alert, so the picker itself is not
        // shown; only its operator/threshold half is reused.
        validator_type: "operator" as const,
        validator_config: { op, threshold: Number(threshold) },
      };
    }

    // Only the type's own block is sent. The server drops the other one anyway,
    // but sending a chart_id on an alert would make the request say something
    // the user did not mean.
    if (isAlert) {
      return {
        ...shared,
        type: "alert" as const,
        // `null`, never `""`. An empty string is not a UUID, so serde rejects
        // the whole body with a bare 422 "Unprocessable Entity" BEFORE the
        // server's own validation runs — the user loses the message that names
        // the field ("an alert needs a database_id") and gets an HTTP status
        // instead. Omitting the value lets the real validator answer.
        database_id: databaseId || databases?.[0]?.id || null,
        sql,
        validator_type: validatorType,
        validator_config:
          validatorType === "operator"
            ? { op, threshold: Number(threshold) }
            : {},
        // Optional on an alert: when set, the fired mail attaches a rendering
        // of this object beside the triggering rows.
        ...(attachTarget
          ? {
              report_format: reportFormat,
              ...(targetKind === "dashboard"
                ? { dashboard_id: defaultTarget || null }
                : { chart_id: defaultTarget || null }),
            }
          : { chart_id: null, dashboard_id: null }),
      };
    }
    // Every target as one list, primary first — the server stores page order
    // from this array, so what the editor shows top-to-bottom is what the PDF
    // pages read.
    const allTargets = [defaultTarget, ...extraTargets]
      .filter(Boolean)
      .map((id) => ({ kind: targetKind, object_id: id }));

    return {
      ...shared,
      type: "report" as const,
      report_format: allTargets.length > 1 ? "pdf" : reportFormat,
      targets: allTargets,
      // Same reason as `database_id` above: an empty target must be absent,
      // not an empty string.
      ...(targetKind === "dashboard"
        ? { dashboard_id: defaultTarget || null }
        : { chart_id: defaultTarget || null }),
    };
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    // `required` cannot be used here: a section the user is not looking at is
    // unmounted, and the browser refuses to submit while silently failing to
    // focus a control it cannot see. Check it ourselves and take them there.
    if (!name.trim()) {
      onSectionChange?.("general");
      setError("A name is required.");
      return;
    }
    const body = buildBody();
    try {
      if (scheduleId) {
        // `type` is immutable server-side and absent from the update body.
        const { type: _omit, ...rest } = body as Record<string, unknown>;
        await update.mutateAsync({ id: scheduleId, body: rest as never });
      } else {
        await create.mutateAsync(body as never);
      }
      // Saving navigates back to the list, so without this the write lands with
      // no acknowledgement at all — every other mutation in this app says what
      // it did, and a silent success reads as a lost click.
      toast.success(scheduleId ? "Saved" : `${isAlert ? "Alert" : "Report"} created`);
      onDone();
    } catch (err) {
      // The server's message names the field to fix ("an alert needs sql",
      // "invalid crontab: …"); a generic "could not save" would throw that away.
      const body = (err as { body?: { error?: string } }).body;
      setError(body?.error ?? (err as Error).message ?? "Could not save");
    }
  }

  const pending = create.isPending || update.isPending;
  useEffect(() => {
    onBusyChange?.(pending);
  }, [pending, onBusyChange]);

  /**
   * Dirtiness by COMPARISON, not by listening for change events.
   *
   * A `change` handler on the form marks it dirty the moment Radix's Select and
   * Checkbox dispatch their own events during hydration — so merely opening a
   * schedule and leaving raised the unsaved-changes prompt. Snapshotting the
   * body and comparing is what ChartExplore and DatasetDetail already do.
   */
  const baseline = useRef<string | null>(null);
  const current = JSON.stringify(buildBody());
  useEffect(() => {
    if (baseline.current === null) {
      // Wait for hydration to LAND before snapshotting. The effect above sets
      // the fields from `existing`, but its `setState` calls only take effect
      // on the next render — snapshot too early and the loaded values read as
      // edits, so the page would warn about unsaved changes on the way out of
      // a schedule nobody touched.
      if (scheduleId && existing && name !== existing.name) return;
      if (scheduleId && !existing) return;
      baseline.current = current;
      onDirtyChange?.(false);
      return;
    }
    onDirtyChange?.(current !== baseline.current);
  }, [current, existing, scheduleId, name, onDirtyChange]);

  return (
    /* ONE column, and NO width of its own — the PAGE sets the measure, so the
       header, the tab bar, this form and the action row all share one edge.
       When the form carried its own `max-w-3xl` it ended short of the tabs
       above it, which reads as a rendering fault rather than a choice.

       Two columns were tried and are wrong for this form: a grid pads a short
       section out to the height of the tall one beside it, and CSS `columns`
       fights `flex` and clips. A twenty-field form reads better as one column
       anyway — the eye never has to decide which side continues the thought. */
    <form id={FORM_ID} onSubmit={submit} className="flex flex-col gap-4">
      {error && (
        <div ref={errorRef}>
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        </div>
      )}

      {section === "general" && (
      <ConfigSection
        level={2}
        title="General"
        description="What this is called, who owns it, and whether it runs at all."
      >
        <ConfigTable>
          <ConfigTableRow
            label="Name"
            htmlFor="sched-name"
            details="Shown in the list, and interpolated into the default email subject."
          >
            <Input id="sched-name" value={name} onChange={(e) => setName(e.target.value)} />
          </ConfigTableRow>

          <ConfigTableRow
            label="Description"
            htmlFor="sched-desc"
            details="Optional. A sentence for whoever finds this later."
          >
            <Input
              id="sched-desc"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </ConfigTableRow>

          {/* No `htmlFor`: the control is a combobox, not an element with an
              id, and it carries its own `aria-label`. A label pointing at an
              id that no longer exists is worse than none — it silently stops
              focusing anything. */}
          <ConfigTableRow
            label="Owners"
            details="The query runs with an owner's permissions. Leave empty and you become the owner; naming several keeps it running when one person leaves."
          >
            {/* A picker, not a `<select multiple>`. The raw control was tolerable
                while it sat in an Advanced tab nobody opened; in General it is
                the third thing you see, and it cannot show who is selected
                without scrolling a grey box. `creatable` is off — an owner has
                to be a member of this workspace, so a typed string is never a
                valid one. */}
            <TagCombobox
              values={owners}
              onChange={setOwners}
              ariaLabel="Owners"
              placeholder={owners.length ? "Add another owner…" : "Search people…"}
              creatable={false}
              queryKey={["alerts", "owner-suggest", ws]}
              labelFor={(id) => memberName(id)}
              suggest={async (q) => {
                const needle = q.trim().toLowerCase();
                return userMembers
                  .filter((m) =>
                    needle ? (m.principal_name ?? m.principal_id).toLowerCase().includes(needle) : true,
                  )
                  .map((m) => ({
                    value: m.principal_id,
                    label: m.principal_name ?? m.principal_id,
                  }));
              }}
            />
          </ConfigTableRow>

          <ConfigTableRow
            label="Active"
            htmlFor="sched-active"
            details="Uncheck to stop it running without deleting it; its history is kept."
          >
            <Checkbox
              id="sched-active"
              checked={active}
              onCheckedChange={(v) => setActive(v === true)}
            />
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>
      )}

      {section === "what" && (isAlert ? (
        <ConfigSection
          level={2}
          title="Alert condition"
          description="The query to run, and what about its result makes this fire."
        >
          <ConfigTable valueClassName="w-52 lg:w-80">
            <ConfigTableRow
              label="Database"
              htmlFor="sched-db"
              details="Which connection the query runs against."
            >
              <Select value={databaseId || databases?.[0]?.id || ""} onValueChange={setDatabaseId}>
                <SelectTrigger id="sched-db">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(databases ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>
                      {d.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </ConfigTableRow>

            <ConfigTableRow
              label="SQL"
              htmlFor="sched-sql"
              details="The first column of the first row is the value the condition is tested against."
            >
              <div className="space-y-1.5">
                <Textarea
                  id="sched-sql"
                  rows={4}
                  value={sql}
                  onChange={(e) => setSql(e.target.value)}
                  placeholder="SELECT count(*) FROM orders WHERE created_at > now() - interval '1' day"
                />
                {(databases ?? []).length === 0 && (
                  <p className="text-xs text-destructive">
                    This workspace has no database connection yet. Add one under Workspace
                    settings &rarr; Engines before creating an alert.
                  </p>
                )}
              </div>
            </ConfigTableRow>

            <ConfigTableRow
              label="Condition"
              htmlFor="sched-validator"
              details="What about the result makes this fire."
            >
              <div className="space-y-2">
                <Select
                  value={validatorType}
                  onValueChange={(v) => setValidatorType(v as "not_null" | "operator")}
                >
                  <SelectTrigger id="sched-validator">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="not_null">Value is not null and not zero</SelectItem>
                    <SelectItem value="operator">Value meets a comparison</SelectItem>
                  </SelectContent>
                </Select>
                {validatorType === "operator" && (
                  <div className="flex gap-2">
                    <Select value={op} onValueChange={setOp}>
                      <SelectTrigger aria-label="Operator">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {OPERATORS.map((o) => (
                          <SelectItem key={o} value={o}>
                            {o}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Input
                      aria-label="Threshold"
                      value={threshold}
                      onChange={(e) => setThreshold(e.target.value)}
                    />
                  </div>
                )}
              </div>
            </ConfigTableRow>
          </ConfigTable>
        </ConfigSection>
      ) : isDataapiAlert ? (
        <ConfigSection
          level={2}
          title="Alert condition"
          description="Which Data API traffic this watches, and what about it makes this fire."
        >
          <ConfigTable valueClassName="w-52 lg:w-80">
            <ConfigTableRow
              label="Endpoint"
              htmlFor="sched-dataapi-endpoint"
              details="Which endpoint's traffic this watches. Whole workspace combines every endpoint's traffic into one measurement — choose it deliberately, not by leaving this blank; every save sends this field either way."
            >
              <div className="space-y-2">
                <Select
                  value={dataapiEndpointId || WHOLE_WORKSPACE}
                  onValueChange={(v) => setDataapiEndpointId(v === WHOLE_WORKSPACE ? "" : v)}
                >
                  <SelectTrigger id="sched-dataapi-endpoint">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                  <SelectItem value={WHOLE_WORKSPACE}>Whole workspace</SelectItem>
                  {/* The stored endpoint may no longer be in the (untrashed-only)
                      list — trashed, or a workspace the caller can no longer
                      read. Injected as its own option so the picker keeps
                      showing the REAL stored value rather than silently falling
                      back to "Whole workspace" and having an untouched Save
                      widen the watch to the entire workspace. Same precedent as
                      the Schedule section's timezone picker below. */}
                  {dataapiEndpointId && !endpoints.some((e) => e.id === dataapiEndpointId) && (
                    <SelectItem value={dataapiEndpointId}>
                      {dataapiEndpointId} (not visible — trashed or inaccessible)
                    </SelectItem>
                  )}
                  {endpoints.map((e) => (
                    <SelectItem key={e.id} value={e.id}>
                      {e.name}
                    </SelectItem>
                  ))}
                  </SelectContent>
                </Select>
                {/* A FAILED fetch is not "this workspace has no endpoints" — the
                    picker still renders (it always has "Whole workspace"), so
                    without this a 403 or a network error is indistinguishable
                    from a genuinely empty list (ui-conventions.md §2.2). Editing
                    is already safe either way (the injected stored-value option
                    above keeps a watch from silently widening), but a NEW
                    schedule's default is already the broadest scope, so a
                    creator who cannot see the real endpoint list needs to be
                    told, not left to assume none exist. */}
                {endpointsQuery.isError && (
                  <Alert variant="destructive">
                    <AlertDescription>
                      Could not load this workspace&apos;s Data API endpoints.{" "}
                      {(endpointsQuery.error as unknown as Error)?.message ??
                        "Try again in a moment."}
                    </AlertDescription>
                  </Alert>
                )}
              </div>
            </ConfigTableRow>

            <ConfigTableRow
              label="Metric"
              htmlFor="sched-dataapi-metric"
              details={DATAAPI_METRIC_HELP[dataapiMetric]}
            >
              <Select value={dataapiMetric} onValueChange={setDataapiMetric}>
                <SelectTrigger id="sched-dataapi-metric">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={DataapiMetric.error_rate}>Error rate</SelectItem>
                  <SelectItem value={DataapiMetric.error_count}>Error count</SelectItem>
                  <SelectItem value={DataapiMetric.call_count}>Call count</SelectItem>
                  <SelectItem value={DataapiMetric.p50_duration_ms}>p50 latency</SelectItem>
                  <SelectItem value={DataapiMetric.p95_duration_ms}>p95 latency</SelectItem>
                  <SelectItem value={DataapiMetric.error_code_count}>Count of a specific error code</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>

            {dataapiMetric === DataapiMetric.error_code_count && (
              <ConfigTableRow
                label="Error code"
                htmlFor="sched-dataapi-error-code"
                details="Matched exactly against the error_code recorded on each call — for example rate_limited. Required for this metric; the schema rejects it set on any other."
              >
                <Input
                  id="sched-dataapi-error-code"
                  value={dataapiErrorCode}
                  onChange={(e) => setDataapiErrorCode(e.target.value)}
                  placeholder="e.g. rate_limited"
                />
              </ConfigTableRow>
            )}

            <ConfigTableRow
              label="Window (seconds)"
              htmlFor="sched-dataapi-window"
              details="How far back from now each run measures, up to 30 days (2,592,000 seconds). Unlike the Activity page's own range picker, this alert is never shown on a screen — so if the window reaches past this workspace's access-log retention, the measurement is silently truncated to whatever traffic survived instead of being disabled."
            >
              <Input
                id="sched-dataapi-window"
                value={dataapiWindowS}
                onChange={(e) => setDataapiWindowS(e.target.value)}
                inputMode="numeric"
                type="number"
                min={1}
                max={2592000}
              />
            </ConfigTableRow>

            {/* Reused from the engine alert's own Condition row above: same
                operator list, same aria-label on Threshold. There is no
                not_null/operator picker here — schedule_dataapi_needs_source
                (migration 0128) pins validator_type to 'operator' for every
                dataapi_alert, so only that half of the pattern applies. */}
            <ConfigTableRow
              label="Condition"
              htmlFor="sched-dataapi-op"
              details="What about the measured value makes this fire."
            >
              <div className="flex gap-2">
                <Select value={op} onValueChange={setOp}>
                  <SelectTrigger id="sched-dataapi-op">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {OPERATORS.map((o) => (
                      <SelectItem key={o} value={o}>
                        {o}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  aria-label="Threshold"
                  value={threshold}
                  onChange={(e) => setThreshold(e.target.value)}
                />
              </div>
            </ConfigTableRow>
          </ConfigTable>

          <p className="text-xs text-muted-foreground">
            Try-it trials are never counted — every metric here excludes them, the same as the
            Activity page. If the alert's owner loses read access to the endpoint, the run is
            recorded as error, not noop.
          </p>
        </ConfigSection>
      ) : (
        <ConfigSection
          level={2}
          title="Contents"
          description="Which object to deliver, and in what format."
        >
          <ConfigTable>
            <ConfigTableRow
              label="Deliver"
              htmlFor="sched-target-kind"
              details="Whether this delivers a dashboard or a single chart."
            >
              <Select
                value={targetKind}
                onValueChange={(v) => {
                  setTargetKind(v as "dashboard" | "chart");
                  // Clear the id: a chart id in the dashboard slot is exactly the
                  // "both targets" shape the server rejects.
                  setTargetId("");
                }}
              >
                <SelectTrigger id="sched-target-kind">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="dashboard">A dashboard</SelectItem>
                  <SelectItem value="chart">A chart</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>

            <ConfigTableRow
              label={targetKind === "dashboard" ? "Dashboard" : "Chart"}
              htmlFor="sched-target"
              details="The object this report delivers."
            >
              <Select value={defaultTarget} onValueChange={setTargetId}>
                <SelectTrigger id="sched-target">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                    <SelectItem key={o.id} value={o.id}>
                      {o.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </ConfigTableRow>

            {extraTargets.map((id, i) => (
              <ConfigTableRow
                key={i}
                label={`Also include ${targetKind === "dashboard" ? "dashboard" : "chart"}`}
                htmlFor={`sched-extra-${i}`}
                details="Delivered as an additional page in the same document."
              >
                <div className="flex items-end gap-2">
                  <Select
                    value={id}
                    onValueChange={(v) =>
                      setExtraTargets(extraTargets.map((x, j) => (j === i ? v : x)))
                    }
                  >
                    <SelectTrigger id={`sched-extra-${i}`} className="flex-1">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                        <SelectItem key={o.id} value={o.id}>{o.name}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setExtraTargets(extraTargets.filter((_, j) => j !== i))}
                  >
                    Remove
                  </Button>
                </div>
              </ConfigTableRow>
            ))}

            <ConfigTableRow
              label="Format"
              htmlFor="sched-format"
              details={
                extraTargets.length > 0
                  ? "Several objects are always delivered as a multi-page PDF — a PNG has no second page."
                  : "PNG and PDF are produced by the rendering service; CSV and text are read straight from the query."
              }
            >
              <Select value={reportFormat} onValueChange={setReportFormat}>
                <SelectTrigger id="sched-format">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="png">PNG image</SelectItem>
                  <SelectItem value="pdf">PDF</SelectItem>
                  <SelectItem value="csv">CSV data</SelectItem>
                  <SelectItem value="text">Text</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>
          </ConfigTable>

          <div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => {
                const pool = (targetKind === "dashboard" ? dashboards : charts) ?? [];
                const first = pool.find(
                  (o) => o.id !== defaultTarget && !extraTargets.includes(o.id),
                );
                if (first) setExtraTargets([...extraTargets, first.id]);
              }}
            >
              + Add another
            </Button>
          </div>
        </ConfigSection>
      ))}

      {section === "schedule" && (
      <ConfigSection
        level={2}
        title="Schedule"
        description="When it runs, in which timezone."
      >
        <ConfigTable>
          <ConfigTableGroup label="When it runs" />

          {/* `Cron expression` and the picker (`Repeat`/`Every`/`On`/`On day`/`At`)
              are two alternative ways to say the same thing, and `rawCron`
              decides which one is showing — carried over unchanged from the
              file. The picker reads as one sentence ("every 2 weeks on
              Tuesday at 09:00") and its own controls are individually narrow,
              so it is kept as ONE row rather than split into five: splitting
              it would separate the `flex flex-wrap` layout from the row that
              gives it room, and gains nothing since none of its sub-controls
              have help text of their own. */}
          {rawCron ? (
            <ConfigTableRow
              label="Cron expression"
              htmlFor="sched-cron"
              details="Five fields, as in crontab: minute hour day-of-month month day-of-week."
            >
              <div className="space-y-1.5">
                <Input
                  id="sched-cron"
                  value={crontab}
                  onChange={(e) => setCrontab(e.target.value)}
                  placeholder="0 9 * * *"
                />
                <div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="h-7 px-2 text-xs"
                    onClick={() => {
                      const parsed = fromCron(crontab);
                      if (parsed) setSched(parsed);
                      setRawCron(false);
                    }}
                  >
                    Use the simple picker
                  </Button>
                  {fromCron(crontab) === null && (
                    <span className="ml-1 text-xs text-muted-foreground">
                      (this expression can&apos;t be shown in the picker — switching will
                      change it)
                    </span>
                  )}
                </div>
              </div>
            </ConfigTableRow>
          ) : (
            <ConfigTableRow
              label="Repeat"
              htmlFor="sched-period"
              details="How often this runs, and the day or time of day it runs on."
            >
              <div className="flex flex-wrap items-end gap-2">
                <div className="flex flex-col gap-1.5">
                  <Select
                    value={sched.period}
                    onValueChange={(v) =>
                      setSched({
                        ...sched,
                        period: v as StructuredSchedule["period"],
                      })
                    }
                  >
                    <SelectTrigger id="sched-period">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="minutes">Every N minutes</SelectItem>
                      <SelectItem value="hourly">Hourly</SelectItem>
                      <SelectItem value="daily">Daily</SelectItem>
                      <SelectItem value="weekly">Weekly</SelectItem>
                      <SelectItem value="monthly">Monthly</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                {/* Two mutually exclusive fields share htmlFor="sched-every" —
                    `period` can never be both "minutes" and "hourly" at once,
                    so only one of these two ever mounts. Both keep the id and
                    both keep their own branch unchanged from the file. */}
                {sched.period === "minutes" && (
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="sched-every">Every</Label>
                    <Select
                      value={String(sched.every)}
                      onValueChange={(v) => setSched({ ...sched, every: Number(v) })}
                    >
                      <SelectTrigger id="sched-every">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {[1, 2, 5, 10, 15, 20, 30].map((n) => (
                          <SelectItem key={n} value={String(n)}>{n} minutes</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                )}

                {sched.period === "hourly" && (
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="sched-every">Every</Label>
                    <Select
                      value={String(sched.every)}
                      onValueChange={(v) => setSched({ ...sched, every: Number(v) })}
                    >
                      <SelectTrigger id="sched-every">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {[1, 2, 3, 4, 6, 8, 12].map((n) => (
                          <SelectItem key={n} value={String(n)}>{n === 1 ? "hour" : `${n} hours`}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                )}

                {sched.period === "weekly" && (
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="sched-weekday">On</Label>
                    <Select
                      value={String(sched.weekday)}
                      onValueChange={(v) => setSched({ ...sched, weekday: Number(v) })}
                    >
                      <SelectTrigger id="sched-weekday">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {WEEKDAYS.map((d, i) => (
                          <SelectItem key={d} value={String(i)}>{d}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                )}

                {sched.period === "monthly" && (
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="sched-day">On day</Label>
                    <Select
                      value={String(sched.day)}
                      onValueChange={(v) => setSched({ ...sched, day: Number(v) })}
                    >
                      <SelectTrigger id="sched-day">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
                          <SelectItem key={d} value={String(d)}>{d}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                )}

                {sched.period !== "minutes" && (
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="sched-time">At</Label>
                    <div className="flex items-center gap-1">
                      {sched.period !== "hourly" && (
                        <>
                          <Select
                            value={String(sched.hour)}
                            onValueChange={(v) => setSched({ ...sched, hour: Number(v) })}
                          >
                            <SelectTrigger id="sched-time" aria-label="Hour">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              {Array.from({ length: 24 }, (_, h) => h).map((h) => (
                                <SelectItem key={h} value={String(h)}>{String(h).padStart(2, "0")}</SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <span className="text-sm text-muted-foreground">:</span>
                        </>
                      )}
                      <Select
                        value={String(sched.minute)}
                        onValueChange={(v) => setSched({ ...sched, minute: Number(v) })}
                      >
                        <SelectTrigger aria-label="Minute">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {[0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55].map((m) => (
                            <SelectItem key={m} value={String(m)}>{String(m).padStart(2, "0")}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                )}
              </div>
            </ConfigTableRow>
          )}

          {/* Timezone was, and stays, part of the picker branch only — the raw
              cron field has never offered a way to change it in this editor,
              and this refactor is not the place to change that. */}
          {!rawCron && (
            <ConfigTableRow
              label="Timezone"
              htmlFor="sched-tz"
              details="Runs are scheduled in this zone. It also renders the {{date}}/{{time}} placeholders in the subject — the message body itself has no dates in it."
            >
              <TimezoneSelect id="sched-tz" value={timezone} onChange={setTimezone} />
            </ConfigTableRow>
          )}

          <ConfigTableGroup label="Limits" />

          <ConfigTableRow
            label="Grace period (seconds)"
            htmlFor="sched-grace"
            details="After it fires, stays quiet for this long so a condition that stays true doesn't mail every tick. Any earlier firing counts, not only a repeat of this one — the run still happens, only delivery is skipped. Reports ignore this."
          >
            <Input
              id="sched-grace"
              value={gracePeriod}
              onChange={(e) => setGracePeriod(e.target.value)}
              inputMode="numeric"
            />
          </ConfigTableRow>

          <ConfigTableRow
            label="Working timeout (seconds)"
            htmlFor="sched-timeout"
            details="How long a run can go quiet before it's reclaimed, in case the worker running it died mid-run."
          >
            <Input
              id="sched-timeout"
              value={workingTimeout}
              onChange={(e) => setWorkingTimeout(e.target.value)}
              inputMode="numeric"
            />
          </ConfigTableRow>

          <ConfigTableRow
            label="Run log retention (days)"
            htmlFor="sched-retention"
            details="How long a finished run stays in the log."
          >
            <Input
              id="sched-retention"
              value={logRetention}
              onChange={(e) => setLogRetention(e.target.value)}
              inputMode="numeric"
            />
          </ConfigTableRow>
        </ConfigTable>

        {/* A computed summary, not a field with a value of its own — it names
            no single row (it reads the whole picker AND the timezone), so it
            cannot be one row's Details. It follows the precedent set by the
            Contents section's "+ Add another" button and the Notification
            section's report-preview paragraph: a bare, non-field element is
            illegal inside a table body, so it renders below the table. */}
        {!rawCron && (
          <p className="text-xs text-muted-foreground">
            Runs <code>{toCron(sched)}</code> in {timezone}.{" "}
            <button
              type="button"
              className="underline underline-offset-2"
              onClick={() => {
                setCrontab(toCron(sched));
                setRawCron(true);
              }}
            >
              Write the cron expression instead
            </button>
          </p>
        )}
      </ConfigSection>
      )}

      {section === "notification" && (
      <ConfigSection
        level={2}
        title="Notification"
        description="Who hears about it, through which channels, and what the message says."
      >
        {/* Destinations first, content second: "who am I telling" is the
            question a person answers before "what does it say", and the
            content fields below describe the email that the Email
            destination produces. */}
        <DestinationList value={destinations} onChange={setDestinations} />

        <ConfigTable valueClassName="w-52 lg:w-80">
          <ConfigTableGroup label="Message" />

          <ConfigTableRow
            label="Email subject"
            htmlFor="sched-subject"
            details="Leave blank to use the default subject."
          >
            {/* A subject fixed for every edition collapses thirty mails into
                one thread and nobody can find yesterday's, which is why the
                placeholders exist and why the preview is here rather than in a
                tooltip. The insert buttons stay beside the input rather than
                in Details: they act on the field, so they belong with it. */}
            <div className="space-y-1.5">
              <Input
                id="sched-subject"
                ref={subjectRef}
                value={emailSubject}
                onChange={(e) => setEmailSubject(e.target.value)}
                placeholder={
                  isAlert || isDataapiAlert
                    ? `[egret] ${name || "name"} triggered`
                    : `[egret] ${name || "name"}`
                }
              />
              <p className="text-xs text-muted-foreground">
                Placeholders:{" "}
                {SUBJECT_TOKENS.map((t, i) => (
                  <span key={t.token}>
                    {i > 0 && ", "}
                    <button
                      type="button"
                      className="font-mono underline hover:text-foreground"
                      onClick={() => insertToken(t.token)}
                      title={`Insert ${t.hint}`}
                    >{`{{${t.token}}}`}</button>
                  </span>
                ))}
                .
              </p>
              {emailSubject.includes("{{") && (
                <p className="text-xs text-muted-foreground">
                  Sends as{" "}
                  <span className="font-medium text-foreground">
                    {renderSubject(emailSubject, name || "name")}
                  </span>
                </p>
              )}
            </div>
          </ConfigTableRow>

          <ConfigTableRow
            label="Opening note"
            htmlFor="sched-message"
            details="Above the result, at the top of the notification. Plain text."
          >
            <Textarea
              id="sched-message"
              rows={2}
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              placeholder="Anything the recipient should know — why this exists, who to ask."
            />
          </ConfigTableRow>

          <ConfigTableRow
            label="Closing note"
            htmlFor="sched-message-footer"
            details="Below the result, before the link back to egret. Plain text."
          >
            <Textarea
              id="sched-message-footer"
              rows={2}
              value={messageFooter}
              onChange={(e) => setMessageFooter(e.target.value)}
              placeholder="What to do about it — who to contact, where to look next."
            />
          </ConfigTableRow>

          {/* The group only exists for an alert: a report's attachment is
              already chosen in Contents, so there is nothing here to edit and
              no row to head. Guarding the group row on the same condition as
              the row beneath it (rather than on `attachTarget`) keeps it from
              ever heading zero rows — "Also show a chart or dashboard" is
              unconditional once `isAlert` is true. */}
          {isAlert && (
            <>
              <ConfigTableGroup label="Attachment" />

              {/* This control used to sit at the foot of the Condition
                  section, which is about the query and the firing rule; a
                  user looking for "can I send the chart too?" never looked
                  there and concluded the feature did not exist. */}
              <ConfigTableRow
                label="Also show a chart or dashboard"
                htmlFor="sched-attach"
                details="Renders the chart or dashboard picked below and attaches it alongside the numbers."
              >
                <div className="flex flex-col gap-2">
                  <Checkbox
                    id="sched-attach"
                    checked={attachTarget}
                    onCheckedChange={(v) => setAttachTarget(v === true)}
                  />
                  {attachTarget && (
                    <div className="flex flex-col gap-2">
                      <Select
                        value={targetKind}
                        onValueChange={(v) => {
                          setTargetKind(v as "dashboard" | "chart");
                          setTargetId("");
                        }}
                      >
                        <SelectTrigger aria-label="Attachment object type">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="dashboard">A dashboard</SelectItem>
                          <SelectItem value="chart">A chart</SelectItem>
                        </SelectContent>
                      </Select>
                      <Select value={defaultTarget} onValueChange={setTargetId}>
                        <SelectTrigger aria-label="Attachment object">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                            <SelectItem key={o.id} value={o.id}>
                              {o.name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <Select value={reportFormat} onValueChange={setReportFormat}>
                        <SelectTrigger aria-label="Attachment format">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="png">PNG image, shown here in the message</SelectItem>
                          <SelectItem value="pdf">PDF, as a file</SelectItem>
                        </SelectContent>
                      </Select>
                      <p className="text-xs text-muted-foreground">
                        If it cannot be rendered the alert still sends, with the rows alone.
                      </p>
                    </div>
                  )}
                </div>
              </ConfigTableRow>

              {attachTarget && (
                <>
                  <ConfigTableRow
                    label="Image width (px)"
                    htmlFor="sched-width"
                    details="Overrides the rendered image's default width — only takes effect if height is set too; leave either blank and both fall back to the default size."
                  >
                    <Input
                      id="sched-width"
                      value={customWidth}
                      onChange={(e) => setCustomWidth(e.target.value)}
                      placeholder="default"
                      inputMode="numeric"
                    />
                  </ConfigTableRow>
                  <ConfigTableRow
                    label="Image height (px)"
                    htmlFor="sched-height"
                    details="Overrides the rendered image's default height — only takes effect if width is set too; leave either blank and both fall back to the default size."
                  >
                    <Input
                      id="sched-height"
                      value={customHeight}
                      onChange={(e) => setCustomHeight(e.target.value)}
                      placeholder="default"
                      inputMode="numeric"
                    />
                  </ConfigTableRow>
                </>
              )}
            </>
          )}

          <ConfigTableGroup label="Delivery" />

          <ConfigTableRow
            label="Don't send when there are no rows"
            htmlFor="sched-skip-empty"
            details="The run still happens and is recorded as noop — only the notification is skipped. Reports and Data API alerts don't read this setting; a Data API alert with nothing to report already records noop on its own, from the measured value alone."
          >
            <Checkbox
              id="sched-skip-empty"
              checked={skipIfEmpty}
              onCheckedChange={(v) => setSkipIfEmpty(v === true)}
            />
          </ConfigTableRow>
        </ConfigTable>

        {/* What the message always carries, not a field itself — it has no
            control of its own, so (like the report preview below) it cannot
            be a row and sits outside the table instead. This used to be a
            centred paragraph above the attachment checkbox; it described the
            message body, not the checkbox, so it did not belong as that
            row's Details. */}
        {isAlert && (
          <p className="text-xs text-muted-foreground">
            The triggering rows are always attached — as a CSV file to email. A webhook receives
            them as JSON in the payload instead; Slack does not receive them at all.
          </p>
        )}

        {/* A dataapi_alert has no query result set to attach — the measured
            value IS the whole answer (runner.rs's evaluate_and_deliver_dataapi
            _alert doc comment) — so it gets neither the CSV note above nor
            the report preview below, both of which describe things it does
            not have. */}
        {isDataapiAlert && (
          <p className="text-xs text-muted-foreground">
            There are no query rows to attach — the measured value is the whole result, shown
            in the message itself.
          </p>
        )}

        {/* A report's own preview text, not a field — it has no value to
            edit, so it cannot be a row. It used to sit between the two notes,
            which the table's grouping (Message, then Attachment) no longer
            has room for; it moves below the table for the same reason the
            "+ Add another" button did in the Contents section: a bare,
            non-field element is illegal inside a table body. */}
        {!isAlert && !isDataapiAlert && (
          <p className="rounded-md border border-dashed px-3 py-3 text-center text-xs text-muted-foreground">
            {reportFormat === "png"
              ? "The rendered image goes here, inside the message."
              : "The report goes here — csv and pdf arrive as a file, text is shown inline."}
          </p>
        )}
      </ConfigSection>
      )}


    </form>
  );
}
