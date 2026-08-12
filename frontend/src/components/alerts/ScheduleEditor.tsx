import { useEffect, useRef, useState } from "react";
import {
  useSchedule,
  useScheduleMutations,
  type ScheduleType,
} from "@/api/alerts";
import { useDatabases } from "@/api/databases";
import { useDashboards } from "@/api/dashboards";
import { useCharts } from "@/api/charts";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertCircle } from "lucide-react";

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
  onDone,
}: {
  ws: string;
  type: ScheduleType;
  scheduleId: string | null;
  onDone: () => void;
}) {
  const { data: existing } = useSchedule(ws, scheduleId);
  const { create, update } = useScheduleMutations(ws);
  const { data: databases } = useDatabases(ws);
  const { data: dashboards } = useDashboards(ws);
  const { data: charts } = useCharts(ws);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [crontab, setCrontab] = useState("0 9 * * *");
  const [timezone, setTimezone] = useState("UTC");
  const [active, setActive] = useState(true);
  const [sql, setSql] = useState("");
  const [databaseId, setDatabaseId] = useState("");
  const [validatorType, setValidatorType] = useState<"not_null" | "operator">("not_null");
  const [op, setOp] = useState(">");
  const [threshold, setThreshold] = useState("0");
  const [targetKind, setTargetKind] = useState<"dashboard" | "chart">("dashboard");
  const [targetId, setTargetId] = useState("");
  const [reportFormat, setReportFormat] = useState("png");
  const [emails, setEmails] = useState("");
  const [webhooks, setWebhooks] = useState("");
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
    ];

    const shared = {
      name,
      description,
      active,
      crontab,
      timezone,
      recipients,
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
      };
    }
    return {
      ...shared,
      type: "report" as const,
      report_format: reportFormat,
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
      onDone();
    } catch (err) {
      // The server's message names the field to fix ("an alert needs sql",
      // "invalid crontab: …"); a generic "could not save" would throw that away.
      const body = (err as { body?: { error?: string } }).body;
      setError(body?.error ?? (err as Error).message ?? "Could not save");
    }
  }

  const pending = create.isPending || update.isPending;

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      {error && (
        <div ref={errorRef}>
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        </div>
      )}

      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium">General</h3>
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
      </section>

      {isAlert ? (
        <section className="flex flex-col gap-3">
          <h3 className="text-sm font-medium">Alert condition</h3>
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
        </section>
      ) : (
        <section className="flex flex-col gap-3">
          <h3 className="text-sm font-medium">Contents</h3>
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
              Image and PDF delivery needs the rendering service, which is not deployed yet.
            </p>
          </div>
        </section>
      )}

      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium">Schedule</h3>
        <div className="flex gap-2">
          <div className="flex flex-1 flex-col gap-1.5">
            <Label htmlFor="sched-cron">Cron expression</Label>
            <Input
              id="sched-cron"
              value={crontab}
              onChange={(e) => setCrontab(e.target.value)}
              placeholder="0 9 * * *"
            />
          </div>
          <div className="flex flex-1 flex-col gap-1.5">
            <Label htmlFor="sched-tz">Timezone</Label>
            <Input
              id="sched-tz"
              value={timezone}
              onChange={(e) => setTimezone(e.target.value)}
              placeholder="UTC"
            />
          </div>
        </div>
        <p className="text-xs text-muted-foreground">
          Five fields, as in crontab: minute hour day-of-month month day-of-week. Evaluated in
          the timezone above.
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <h3 className="text-sm font-medium">Notification</h3>
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
      </section>

      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" disabled={pending}>
          {scheduleId ? "Save" : "Create"}
        </Button>
      </div>
    </form>
  );
}
