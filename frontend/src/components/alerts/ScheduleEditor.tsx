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
import { ConfigSection } from "@/components/form/ConfigSection";

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
  onDirtyChange,
  onBusyChange,
  onDone,
}: {
  ws: string;
  type: ScheduleType;
  scheduleId: string | null;
  /** A target chosen elsewhere — the Subscribe item on a dashboard or chart. */
  prefill?: { kind: "chart" | "dashboard"; object: string };
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
  const [emails, setEmails] = useState("");
  const [emailSubject, setEmailSubject] = useState("");
  const [owners, setOwners] = useState<string[]>([]);
  const [gracePeriod, setGracePeriod] = useState("14400");
  const [workingTimeout, setWorkingTimeout] = useState("3600");
  const [logRetention, setLogRetention] = useState("90");
  const [customWidth, setCustomWidth] = useState("");
  const [customHeight, setCustomHeight] = useState("");
  /** Alerts only: attach a rendering of this chart/dashboard when it fires. */
  const [attachTarget, setAttachTarget] = useState(false);
  const [webhooks, setWebhooks] = useState("");
  const [slackChannel, setSlackChannel] = useState("");
  const [slackToken, setSlackToken] = useState("");
  const [slackHasToken, setSlackHasToken] = useState(false);
  /**
   * Which webhook URLs already have an HMAC signing secret.
   *
   * The secret itself is never sent to a browser — it lives in the vault and
   * the server reports only `has_secret`. Keeping the SERVER responsible for
   * carrying it across a save (absent means keep, per `vault_secrets`) is why
   * this form can send a bare `{ url }` without unsigning anything.
   */
  const [signedUrls, setSignedUrls] = useState<Set<string>>(new Set());
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
    setEmails(
      existing.recipients
        .filter((r) => r.kind === "email")
        .flatMap((r) => ((r.config as { addresses?: string[] }).addresses ?? []))
        .join(", "),
    );
    const hooks = existing.recipients
      .filter((r) => r.kind === "webhook")
      .map((r) => r.config as { url?: string; has_secret?: boolean })
      .filter((c): c is { url: string; has_secret?: boolean } => Boolean(c.url));
    setWebhooks(hooks.map((c) => c.url).join("\n"));
    const sl = existing.recipients.find((r) => r.kind === "slack")?.config as
      | { channel?: string; has_token?: boolean }
      | undefined;
    setSlackChannel(sl?.channel ?? "");
    setSlackHasToken(Boolean(sl?.has_token));
    setSlackToken("");
    setSignedUrls(new Set(hooks.filter((c) => c.has_secret).map((c) => c.url)));
  }, [existing]);

  /**
   * Bring a rejected save into view.
   *
   * The banner renders at the top of a form tall enough to scroll, and Save
   * sits at the bottom — so without this the server's message ("webhook url
   * must be http(s)") is off-screen and pressing Save looks like it did
   * nothing at all. Applies to every validation error, not only webhooks.
   */
  const errorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (error) errorRef.current?.scrollIntoView({ block: "nearest" });
  }, [error]);

  const isAlert = type === "alert";
  const defaultTarget =
    targetId || (targetKind === "dashboard" ? dashboards?.[0]?.id : charts?.[0]?.id) || "";

  function buildBody() {
    const addresses = emails
      .split(",")
      .map((e) => e.trim())
      .filter(Boolean);
    // One URL per line, not comma-separated like the addresses above: a comma
    // is legal inside a URL's query string.
    const urls = webhooks
      .split("\n")
      .map((u) => u.trim())
      .filter(Boolean);

    // Every destination the schedule has, in one list — the update REPLACES
    // the recipient set, so a kind missing from this array is a kind deleted.
    const recipients = [
      ...(addresses.length ? [{ kind: "email" as const, config: { addresses } }] : []),
      // The URL alone. A `secret` key absent from the body means "keep the one
      // already stored for this URL" (`ScheduleRepo::vault_secrets`), so the
      // browser never has to hold a credential to avoid destroying it.
      ...urls.map((url) => ({ kind: "webhook" as const, config: { url } })),
      // Same three-way rule as the webhook secret: an absent `token` keeps the
      // stored one, so the browser never has to hold a credential in order to
      // avoid destroying it.
      ...(slackChannel.trim()
        ? [
            {
              kind: "slack" as const,
              config: {
                channel: slackChannel.trim(),
                ...(slackToken.trim() ? { token: slackToken.trim() } : {}),
              },
            },
          ]
        : []),
    ];

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

  return (
    /* ONE column, bounded to a readable measure. Two columns were tried and are
       wrong for this form: a grid pads a short section out to the height of the
       tall one beside it, and CSS `columns` fights `flex` and clips. A
       twenty-field form reads better as one column anyway — the eye never has
       to decide which side continues the thought. */
    <form
      id={FORM_ID}
      onSubmit={submit}
      onChange={() => onDirtyChange?.(true)}
      className="flex max-w-3xl flex-col gap-4"
    >
      {error && (
        <div ref={errorRef}>
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        </div>
      )}

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
            required
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

      {isAlert ? (
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
          <div className="flex flex-col gap-1.5 border-t pt-3">
            <div className="flex items-center gap-2">
              <Checkbox
                id="sched-attach"
                checked={attachTarget}
                onCheckedChange={(v) => setAttachTarget(v === true)}
              />
              <Label htmlFor="sched-attach" className="text-sm font-normal">
                Attach a chart or dashboard when it fires
              </Label>
            </div>
            {attachTarget && (
              <div className="mt-1 flex flex-col gap-2 pl-6">
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
                  <option value="png">PNG image</option>
                  <option value="pdf">PDF</option>
                </select>
                <p className="text-xs text-muted-foreground">
                  The triggering rows are always attached as CSV. This adds a picture of the
                  object beside them; if it cannot be rendered the alert still sends.
                </p>
              </div>
            )}
          </div>
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
      )}

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

      <ConfigSection
        level={2}
        title="Notification"
        description="Who hears about it, through which channels, and what the message says."
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-subject">Email subject</Label>
          <Input
            id="sched-subject"
            value={emailSubject}
            onChange={(e) => setEmailSubject(e.target.value)}
            placeholder={
              isAlert ? `[egret] ${name || "name"} triggered` : `[egret] ${name || "name"}`
            }
          />
          <p className="text-xs text-muted-foreground">
            Leave blank to use the default shown above.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-message">Message</Label>
          <Textarea
            id="sched-message"
            rows={3}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            placeholder="Anything the recipient should know — why this exists, who to ask."
          />
          <p className="text-xs text-muted-foreground">
            Shown at the top of the notification, above the result. Plain text.
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
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-emails">Email recipients</Label>
          <Input
            id="sched-emails"
            value={emails}
            onChange={(e) => setEmails(e.target.value)}
            placeholder="on-call@example.com, team@example.com"
          />
          <p className="text-xs text-muted-foreground">
            Comma separated. Sending needs workspace SMTP, which is configured separately.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-slack">Slack channel</Label>
          <div className="flex gap-2">
            <Input
              id="sched-slack"
              className="flex-1"
              value={slackChannel}
              onChange={(e) => setSlackChannel(e.target.value)}
              placeholder="#data-alerts"
            />
            <Input
              aria-label="Slack bot token"
              className="flex-1"
              type="password"
              autoComplete="new-password"
              value={slackToken}
              onChange={(e) => setSlackToken(e.target.value)}
              placeholder={slackHasToken ? "Unchanged" : "xoxb-…"}
            />
          </div>
          <p className="text-xs text-muted-foreground">
            {slackHasToken
              ? "A bot token is stored. Leave it blank to keep it."
              : "Needs a bot token with chat:write, and the bot invited to the channel."}
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="sched-webhooks">Webhook URLs</Label>
          <Textarea
            id="sched-webhooks"
            rows={2}
            value={webhooks}
            onChange={(e) => setWebhooks(e.target.value)}
            placeholder="https://hooks.example.com/egret"
          />
          <p className="text-xs text-muted-foreground">
            One per line, http(s). Each fires a POST with the run's result.
            {signedUrls.size > 0 &&
              ` ${signedUrls.size === 1 ? "One is" : `${signedUrls.size} are`} HMAC-signed; the secret is stored server-side and kept on save.`}
          </p>
        </div>
      </ConfigSection>

      <details className="rounded-lg border">
        <summary className="cursor-pointer p-4 text-sm font-medium">
          Advanced
          <span className="ml-2 font-normal text-muted-foreground">
            owners, timers, retention, image size
          </span>
        </summary>
        <div className="flex flex-col gap-4 border-t p-4">
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
      </details>

    </form>
  );
}
