import { useEffect, useRef, useState } from "react";
import {
  useSchedule,
  useScheduleMutations,
  type ScheduleType,
} from "@/api/alerts";
import { useDatabases } from "@/api/databases";
import { useDashboards } from "@/api/dashboards";
import { useCharts } from "@/api/charts";
import { useWorkspaceMembers } from "@/api/members";
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
import { Textarea } from "@/components/ui/textarea";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertCircle } from "lucide-react";
import { toast } from "sonner";

/** The form's id, so the page header's Save button can submit it from outside. */
export const FORM_ID = "schedule-editor";

/** The sections the editor is split across, in nav order. */
export type SectionKey = "general" | "what" | "schedule" | "notification" | "advanced";

export const SECTIONS: { key: SectionKey; label: (isAlert: boolean) => string }[] = [
  { key: "general", label: () => "General" },
  { key: "what", label: (a) => (a ? "Condition" : "Contents") },
  { key: "schedule", label: () => "Schedule" },
  { key: "notification", label: () => "Notification" },
  { key: "advanced", label: () => "Advanced" },
];
import { ConfigSection } from "@/components/form/ConfigSection";
import { DestinationList } from "./DestinationList";
import { toDestinations, toRecipients, type Destination } from "./destinations";
import { SUBJECT_TOKENS, renderSubject } from "@/lib/subjectTemplate";

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
  const [gracePeriod, setGracePeriod] = useState("14400");
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
      grace_period_s: num(gracePeriod, 14400),
      working_timeout_s: num(workingTimeout, 3600),
      log_retention_days: num(logRetention, 90),
      custom_width: customWidth.trim() ? num(customWidth, 0) : null,
      custom_height: customHeight.trim() ? num(customHeight, 0) : null,
    };

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
        description="What this is called, and whether it runs at all."
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-name">Name</Label>
          <Input
            id="sched-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-desc">Description</Label>
          <Input
            id="sched-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>
        <div className="flex items-center gap-2">
          <Checkbox
            id="sched-active"
            checked={active}
            onCheckedChange={(v) => setActive(v === true)}
          />
          <Label htmlFor="sched-active" className="text-sm font-normal">
            Active
          </Label>
        </div>
      </ConfigSection>
      )}

      {section === "what" && (isAlert ? (
        <ConfigSection
          level={2}
          title="Alert condition"
          description="The query to run, and what about its result makes this fire."
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-db">Database</Label>
            <select
              id="sched-db"
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
              value={databaseId || databases?.[0]?.id || ""}
              onChange={(e) => setDatabaseId(e.target.value)}
            >
              {(databases ?? []).map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-sql">SQL</Label>
            <Textarea
              id="sched-sql"
              rows={4}
              value={sql}
              onChange={(e) => setSql(e.target.value)}
              placeholder="SELECT count(*) FROM orders WHERE created_at > now() - interval '1' day"
            />
            <p className="text-xs text-muted-foreground">
              The first column of the first row is the value the condition is tested against.
            </p>
            {(databases ?? []).length === 0 && (
              <p className="text-xs text-destructive">
                This workspace has no database connection yet. Add one under Workspace
                settings &rarr; Engines before creating an alert.
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-validator">Condition</Label>
            <select
              id="sched-validator"
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
              value={validatorType}
              onChange={(e) => setValidatorType(e.target.value as "not_null" | "operator")}
            >
              <option value="not_null">Value is not null and not zero</option>
              <option value="operator">Value meets a comparison</option>
            </select>
          </div>
          {validatorType === "operator" && (
            <div className="flex gap-2">
              <select
                aria-label="Operator"
                className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                value={op}
                onChange={(e) => setOp(e.target.value)}
              >
                {["<", "<=", ">", ">=", "==", "!="].map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
              <Input
                aria-label="Threshold"
                value={threshold}
                onChange={(e) => setThreshold(e.target.value)}
              />
            </div>
          )}
        </ConfigSection>
      ) : (
        <ConfigSection
          level={2}
          title="Contents"
          description="Which object to deliver, and in what format."
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-target-kind">Deliver</Label>
            <select
              id="sched-target-kind"
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
              value={targetKind}
              onChange={(e) => {
                setTargetKind(e.target.value as "dashboard" | "chart");
                // Clear the id: a chart id in the dashboard slot is exactly the
                // "both targets" shape the server rejects.
                setTargetId("");
              }}
            >
              <option value="dashboard">A dashboard</option>
              <option value="chart">A chart</option>
            </select>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-target">
              {targetKind === "dashboard" ? "Dashboard" : "Chart"}
            </Label>
            <select
              id="sched-target"
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
              value={defaultTarget}
              onChange={(e) => setTargetId(e.target.value)}
            >
              {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          </div>
          {extraTargets.map((id, i) => (
            <div key={i} className="flex items-end gap-2">
              <div className="flex flex-1 flex-col gap-1.5">
                <Label htmlFor={`sched-extra-${i}`}>
                  Also include {targetKind === "dashboard" ? "dashboard" : "chart"}
                </Label>
                <select
                  id={`sched-extra-${i}`}
                  className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                  value={id}
                  onChange={(e) =>
                    setExtraTargets(extraTargets.map((x, j) => (j === i ? e.target.value : x)))
                  }
                >
                  {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                    <option key={o.id} value={o.id}>{o.name}</option>
                  ))}
                </select>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setExtraTargets(extraTargets.filter((_, j) => j !== i))}
              >
                Remove
              </Button>
            </div>
          ))}
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
            {extraTargets.length > 0 && (
              <span className="ml-2 text-xs text-muted-foreground">
                Delivered as one PDF, one page each, in this order.
              </span>
            )}
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-format">Format</Label>
            <select
              id="sched-format"
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
              value={reportFormat}
              onChange={(e) => setReportFormat(e.target.value)}
            >
              <option value="png">PNG image</option>
              <option value="pdf">PDF</option>
              <option value="csv">CSV data</option>
              <option value="text">Text</option>
            </select>
            <p className="text-xs text-muted-foreground">
              {extraTargets.length > 0
                ? "Several objects are always delivered as a multi-page PDF — a PNG has no second page."
                : "PNG and PDF are produced by the rendering service; CSV and text are read straight from the query."}
            </p>
          </div>
        </ConfigSection>
      ))}

      {section === "schedule" && (
      <ConfigSection
        level={2}
        title="Schedule"
        description="When it runs, in which timezone."
      >

        {rawCron ? (
          <>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sched-cron">Cron expression</Label>
              <Input
                id="sched-cron"
                value={crontab}
                onChange={(e) => setCrontab(e.target.value)}
                placeholder="0 9 * * *"
              />
              <p className="text-xs text-muted-foreground">
                Five fields, as in crontab: minute hour day-of-month month day-of-week.
              </p>
            </div>
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
                  (this expression can&apos;t be shown in the picker — switching will change it)
                </span>
              )}
            </div>
          </>
        ) : (
          <>
            <div className="flex flex-wrap items-end gap-2">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="sched-period">Repeat</Label>
                <select
                  id="sched-period"
                  className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                  value={sched.period}
                  onChange={(e) =>
                    setSched({ ...sched, period: e.target.value as StructuredSchedule["period"] })
                  }
                >
                  <option value="minutes">Every N minutes</option>
                  <option value="hourly">Hourly</option>
                  <option value="daily">Daily</option>
                  <option value="weekly">Weekly</option>
                  <option value="monthly">Monthly</option>
                </select>
              </div>

              {sched.period === "minutes" && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="sched-every">Every</Label>
                  <select
                    id="sched-every"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={sched.every}
                    onChange={(e) => setSched({ ...sched, every: Number(e.target.value) })}
                  >
                    {[1, 2, 5, 10, 15, 20, 30].map((n) => (
                      <option key={n} value={n}>{n} minutes</option>
                    ))}
                  </select>
                </div>
              )}

              {sched.period === "hourly" && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="sched-every">Every</Label>
                  <select
                    id="sched-every"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={sched.every}
                    onChange={(e) => setSched({ ...sched, every: Number(e.target.value) })}
                  >
                    {[1, 2, 3, 4, 6, 8, 12].map((n) => (
                      <option key={n} value={n}>{n === 1 ? "hour" : `${n} hours`}</option>
                    ))}
                  </select>
                </div>
              )}

              {sched.period === "weekly" && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="sched-weekday">On</Label>
                  <select
                    id="sched-weekday"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={sched.weekday}
                    onChange={(e) => setSched({ ...sched, weekday: Number(e.target.value) })}
                  >
                    {WEEKDAYS.map((d, i) => (
                      <option key={d} value={i}>{d}</option>
                    ))}
                  </select>
                </div>
              )}

              {sched.period === "monthly" && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="sched-day">On day</Label>
                  <select
                    id="sched-day"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={sched.day}
                    onChange={(e) => setSched({ ...sched, day: Number(e.target.value) })}
                  >
                    {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
                      <option key={d} value={d}>{d}</option>
                    ))}
                  </select>
                </div>
              )}

              {sched.period !== "minutes" && (
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="sched-time">At</Label>
                  <div className="flex items-center gap-1">
                    {sched.period !== "hourly" && (
                      <>
                        <select
                          id="sched-time"
                          aria-label="Hour"
                          className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                          value={sched.hour}
                          onChange={(e) => setSched({ ...sched, hour: Number(e.target.value) })}
                        >
                          {Array.from({ length: 24 }, (_, h) => h).map((h) => (
                            <option key={h} value={h}>{String(h).padStart(2, "0")}</option>
                          ))}
                        </select>
                        <span className="text-sm text-muted-foreground">:</span>
                      </>
                    )}
                    <select
                      aria-label="Minute"
                      className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                      value={sched.minute}
                      onChange={(e) => setSched({ ...sched, minute: Number(e.target.value) })}
                    >
                      {[0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55].map((m) => (
                        <option key={m} value={m}>{String(m).padStart(2, "0")}</option>
                      ))}
                    </select>
                  </div>
                </div>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sched-tz">Timezone</Label>
              <select
                id="sched-tz"
                className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                value={timezone}
                onChange={(e) => setTimezone(e.target.value)}
              >
                {/* A stored zone the browser does not list (an older egret, or a
                    newer IANA name) must stay selectable, or opening the editor
                    would silently move the schedule to another timezone. */}
                {!timezones().includes(timezone) && <option value={timezone}>{timezone}</option>}
                {timezones().map((z) => (
                  <option key={z} value={z}>{z}</option>
                ))}
              </select>
            </div>

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
          </>
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

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-subject">Email subject</Label>
          <Input
            id="sched-subject"
            ref={subjectRef}
            value={emailSubject}
            onChange={(e) => setEmailSubject(e.target.value)}
            placeholder={
              isAlert ? `[egret] ${name || "name"} triggered` : `[egret] ${name || "name"}`
            }
          />
          {/* A subject fixed for every edition collapses thirty mails into
              one thread and nobody can find yesterday's, which is why the
              placeholders exist and why the preview is here rather than in a
              tooltip. */}
          <p className="text-xs text-muted-foreground">
            Leave blank to use the default shown above. Placeholders:{" "}
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

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-message">Opening note</Label>
          <Textarea
            id="sched-message"
            rows={2}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            placeholder="Anything the recipient should know — why this exists, who to ask."
          />
          <p className="text-xs text-muted-foreground">
            Above the result, at the top of the notification. Plain text.
          </p>
        </div>

        {/* Between the two notes, because that is where the delivered object
            physically lands in the mail — "opening note / closing note" reads
            as two adjacent boxes otherwise.

            For an alert this is also WHERE THE CONTROL BELONGS. It used to sit
            at the foot of the Condition section, which is about the query and
            the firing rule; a user looking for "can I send the chart too?"
            never looked there and concluded the feature did not exist. */}
        <div className="rounded-md border border-dashed px-3 py-3">
          {isAlert ? (
            <>
              <p className="text-center text-xs text-muted-foreground">
                The triggering rows go here, always attached as CSV.
              </p>
              <div className="mt-2 flex items-center gap-2">
                <Checkbox
                  id="sched-attach"
                  checked={attachTarget}
                  onCheckedChange={(v) => setAttachTarget(v === true)}
                />
                <Label htmlFor="sched-attach" className="text-sm font-normal">
                  Also show a chart or dashboard
                </Label>
              </div>
              {attachTarget && (
                <div className="mt-2 flex flex-col gap-2 pl-6">
                  <select
                    aria-label="Attachment object type"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={targetKind}
                    onChange={(e) => {
                      setTargetKind(e.target.value as "dashboard" | "chart");
                      setTargetId("");
                    }}
                  >
                    <option value="dashboard">A dashboard</option>
                    <option value="chart">A chart</option>
                  </select>
                  <select
                    aria-label="Attachment object"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={defaultTarget}
                    onChange={(e) => setTargetId(e.target.value)}
                  >
                    {((targetKind === "dashboard" ? dashboards : charts) ?? []).map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                  </select>
                  <select
                    aria-label="Attachment format"
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    value={reportFormat}
                    onChange={(e) => setReportFormat(e.target.value)}
                  >
                    <option value="png">PNG image, shown here in the message</option>
                    <option value="pdf">PDF, as a file</option>
                  </select>
                  <p className="text-xs text-muted-foreground">
                    If it cannot be rendered the alert still sends, with the rows alone.
                  </p>
                </div>
              )}
            </>
          ) : (
            <p className="text-center text-xs text-muted-foreground">
              {reportFormat === "png"
                ? "The rendered image goes here, inside the message."
                : "The report goes here — csv and pdf arrive as a file, text is shown inline."}
            </p>
          )}
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-message-footer">Closing note</Label>
          <Textarea
            id="sched-message-footer"
            rows={2}
            value={messageFooter}
            onChange={(e) => setMessageFooter(e.target.value)}
            placeholder="What to do about it — who to contact, where to look next."
          />
          <p className="text-xs text-muted-foreground">
            Below the result, before the link back to egret. Plain text.
          </p>
        </div>

        <div className="flex items-center gap-2">
          <Checkbox
            id="sched-skip-empty"
            checked={skipIfEmpty}
            onCheckedChange={(v) => setSkipIfEmpty(v === true)}
          />
          <Label htmlFor="sched-skip-empty" className="text-sm font-normal">
            Don&apos;t send when there are no rows
          </Label>
        </div>
      </ConfigSection>
      )}

      {section === "advanced" && (
      <ConfigSection
        level={2}
        title="Advanced"
        description="Ownership, the timers that govern re-firing and stuck runs, and how long the run log is kept."
      >
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-owners">Owners</Label>
            <select
              id="sched-owners"
              multiple
              size={Math.min(5, Math.max(3, userMembers.length))}
              className="rounded-md border border-input bg-background px-2 py-1 text-sm"
              value={owners}
              onChange={(e) =>
                setOwners(Array.from(e.target.selectedOptions).map((o) => o.value))
              }
            >
              {userMembers.map((m) => (
                <option key={m.principal_id} value={m.principal_id}>
                  {m.principal_name ?? m.principal_id}
                </option>
              ))}
            </select>
            <p className="text-xs text-muted-foreground">
              The query runs with an owner&apos;s permissions. Leave empty and you become the
              owner; naming several keeps it running when one person leaves.
            </p>
          </div>

          <div className="flex gap-2">
            <div className="flex flex-1 flex-col gap-1.5">
              <Label htmlFor="sched-grace">Grace period (seconds)</Label>
              <Input
                id="sched-grace"
                value={gracePeriod}
                onChange={(e) => setGracePeriod(e.target.value)}
                inputMode="numeric"
              />
            </div>
            <div className="flex flex-1 flex-col gap-1.5">
              <Label htmlFor="sched-timeout">Working timeout (seconds)</Label>
              <Input
                id="sched-timeout"
                value={workingTimeout}
                onChange={(e) => setWorkingTimeout(e.target.value)}
                inputMode="numeric"
              />
            </div>
          </div>
          <p className="text-xs text-muted-foreground">
            After firing, an alert stays quiet for the grace period so a condition that stays
            true does not mail every tick. The working timeout is when a run that never
            finished is reclaimed.
          </p>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sched-retention">Run log retention (days)</Label>
            <Input
              id="sched-retention"
              value={logRetention}
              onChange={(e) => setLogRetention(e.target.value)}
              inputMode="numeric"
            />
          </div>

          {(!isAlert || attachTarget) && (
            <div className="flex gap-2">
              <div className="flex flex-1 flex-col gap-1.5">
                <Label htmlFor="sched-width">Image width (px)</Label>
                <Input
                  id="sched-width"
                  value={customWidth}
                  onChange={(e) => setCustomWidth(e.target.value)}
                  placeholder="default"
                  inputMode="numeric"
                />
              </div>
              <div className="flex flex-1 flex-col gap-1.5">
                <Label htmlFor="sched-height">Image height (px)</Label>
                <Input
                  id="sched-height"
                  value={customHeight}
                  onChange={(e) => setCustomHeight(e.target.value)}
                  placeholder="default"
                  inputMode="numeric"
                />
              </div>
            </div>
          )}
        </div>
      </ConfigSection>
      )}

    </form>
  );
}
