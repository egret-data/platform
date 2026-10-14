import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import { Copy, Eye, EyeOff, SquarePen } from "lucide-react";
import type { DependsOnDto, ScheduleInfoDto, TriggerEntryDto, WebhookInfoDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { StateChip } from "@/components/workflows/StateChip";
import {
  kindLabel,
  missedLabel,
  statusText,
  webhookNote,
  webhookPath,
  type FlowContext,
} from "@/components/workflows/triggers/triggersModel";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";

/** What the drawer says when `?trigger=` names an id the flow no longer declares (spec §5). */
export const TRIGGER_GONE = "This trigger no longer exists in the flow; reload.";

/** `iso` in `timeZone`, or null when the browser does not know that zone. */
function inZone(iso: string, timeZone: string): string | null {
  try {
    return new Intl.DateTimeFormat("en-GB", { timeZone, dateStyle: "medium", timeStyle: "long" }).format(new Date(iso));
  } catch {
    return null;
  }
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="grid gap-2">
      <h3 className="text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}

/** A `Setting | value` list: one `dt`/`dd` pair per fact. */
function Facts({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
      {items.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

function ScheduleFacts({ schedule, runtimeDisabled }: { schedule: ScheduleInfoDto; runtimeDisabled: boolean }) {
  const next = schedule.next_evaluation_date;
  const zoned = next ? inZone(next, schedule.timezone) : null;
  const items: [string, ReactNode][] = [
    ["Cron", <code key="c" className="font-mono">{schedule.cron ?? "—"}</code>],
    ["Timezone", schedule.timezone],
    [
      "Next run",
      next ? (
        <span className="grid">
          <span>
            {zoned ?? "—"} <span className="text-muted-foreground">(the trigger's timezone)</span>
          </span>
          <span>
            {fullTimestamp(next)} <span className="text-muted-foreground">(your time, {timeAgo(next)})</span>
          </span>
        </span>
      ) : (
        "None: the schedule is off the queue"
      ),
    ],
    ["Last evaluated", schedule.evaluated_at ? `${fullTimestamp(schedule.evaluated_at)} (the tick processed, not when it fired)` : "Never"],
    ["Last error", schedule.last_error ?? "None"],
  ];
  if (schedule.missed_ticks && (runtimeDisabled || schedule.missed_ticks.count > 0)) {
    items.push(["Missed ticks", <span key="m" className="tabular-nums">{missedLabel(schedule.missed_ticks)}</span>]);
  }
  return <Facts items={items} />;
}

function DependsOn({ dependsOn }: { dependsOn: DependsOnDto }) {
  const mode = dependsOn.mode === "AT_LEAST" && dependsOn.min_satisfied != null ? `At least ${dependsOn.min_satisfied}` : dependsOn.mode;
  const w = dependsOn.window;
  return (
    <div className="grid gap-2">
      <Facts
        items={[
          ["Mode", mode],
          [
            "Window",
            w ? (
              <span>
                {w.kind}: {fullTimestamp(w.start)} to {fullTimestamp(w.end)}{" "}
                <span className="text-muted-foreground">(closes {timeAgo(w.end)})</span>
              </span>
            ) : (
              "No window is open"
            ),
          ],
        ]}
      />
      {dependsOn.conditions && dependsOn.conditions.length > 0 ? (
        <ul className="grid gap-1 text-sm" role="list" aria-label="Conditions">
          {dependsOn.conditions.map((c, i) => {
            const flow = [c.namespace, c.flow_id].filter(Boolean).join(".") || c.when || "Any execution";
            const states = c.states && c.states.length > 0 ? ` in ${c.states.join(", ")}` : "";
            return (
              <li key={i} className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-xs">{flow}</span>
                <span className="text-muted-foreground">{states}</span>
                <span className={c.met ? "font-medium text-success" : "text-muted-foreground"}>{c.met ? "Met" : "Not met"}</span>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

/** The path with its last segment, the key, hidden: the key is what lets anyone start the flow. */
const masked = (path: string) => path.replace(/[^/]+$/, "••••••••");

function WebhookFacts({ webhook, webhooksEnabled }: { webhook: WebhookInfoDto; webhooksEnabled: boolean }) {
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(t);
  }, [copied]);
  const path = webhookPath(webhook.url_path);

  const copy = async () => {
    if (!path) return;
    try {
      if (!navigator.clipboard) throw new Error("no clipboard");
      await navigator.clipboard.writeText(path);
      setCopied(true);
    } catch {
      toast.error("The path could not be copied: the browser did not allow it. Reveal it and copy it by hand.");
    }
  };

  return (
    <div className="grid gap-2 text-sm">
      {webhook.key_error ? (
        <p>{webhook.key_error}</p>
      ) : path ? (
        <>
          <code className="break-all rounded bg-muted px-2 py-1 font-mono text-xs" data-testid="webhook-path">
            {revealed ? path : masked(path)}
          </code>
          <div className="flex items-center gap-2">
            <Button type="button" size="sm" variant="outline" aria-pressed={revealed} onClick={() => setRevealed((r) => !r)}>
              {revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
              {revealed ? "Hide" : "Reveal"}
            </Button>
            <Button type="button" size="sm" variant="outline" onClick={() => void copy()}>
              <Copy aria-hidden />
              Copy
            </Button>
            <span aria-live="polite" className="text-xs text-muted-foreground">
              {copied ? "Copied" : ""}
            </span>
          </div>
        </>
      ) : null}
      <p className="text-muted-foreground">{webhookNote(webhooksEnabled)}</p>
      {webhook.unsupported ? <p>egret refuses requests to this webhook: {webhook.unsupported}</p> : null}
    </div>
  );
}

/**
 * One trigger's details (spec §4.2), opened by `?trigger=<id>` (FE-PAGE-2):
 * its YAML, and its kind's state. `entry` undefined with `open` is an id the
 * flow no longer declares.
 */
export function TriggerDetailsDrawer({
  open,
  onClose,
  triggerId,
  entry,
  ctx,
  webhooksEnabled,
  block,
  base,
  onEditInEditor,
}: {
  open: boolean;
  onClose: () => void;
  triggerId: string;
  entry: TriggerEntryDto | undefined;
  ctx: FlowContext;
  webhooksEnabled: boolean;
  /** The trigger's YAML block from the saved draft (`triggerBlock`); null when it is not found. */
  block: string | null;
  /** `/explore/{ws}/workflows`. */
  base: string;
  /** Open the flow's Code at this trigger; absent where the editor is not on the page. */
  onEditInEditor?: () => void;
}) {
  return (
    <Sheet open={open} onOpenChange={(o) => { if (!o) onClose(); }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>
            Trigger <span className="font-mono">{triggerId}</span>
          </SheetTitle>
          <SheetDescription>
            {entry ? (
              <>
                {kindLabel(entry.kind)} · <span className="font-mono">{entry.type}</span>
              </>
            ) : (
              TRIGGER_GONE
            )}
          </SheetDescription>
        </SheetHeader>
        <div className="grid min-h-0 flex-1 content-start gap-5 overflow-auto">
          {entry ? (
            <>
              <p className="text-sm">
                <span className="text-muted-foreground">Status: </span>
                {statusText(entry, ctx)}
              </p>
              <Section title="Definition">
                {block !== null ? (
                  <pre className="overflow-auto rounded bg-muted p-3 font-mono text-xs" aria-label={`YAML of ${triggerId}`}>
                    {block}
                  </pre>
                ) : (
                  <p className="text-sm text-muted-foreground">The saved draft no longer declares this trigger; reload.</p>
                )}
                {onEditInEditor ? (
                  <div>
                    <Button type="button" size="sm" variant="outline" onClick={onEditInEditor}>
                      <SquarePen aria-hidden />
                      Edit in the editor
                    </Button>
                  </div>
                ) : null}
              </Section>
              {entry.schedule ? (
                <Section title="Schedule">
                  <ScheduleFacts schedule={entry.schedule} runtimeDisabled={entry.runtime_disabled} />
                </Section>
              ) : null}
              {entry.flow ? (
                <Section title="Flow trigger">
                  <Facts
                    items={[
                      ["States heard", entry.flow.states.length > 0 ? entry.flow.states.join(", ") : "—"],
                      [
                        "Listening",
                        entry.flow.listening ? "Yes" : `No${entry.flow.not_listening_reason ? `: ${entry.flow.not_listening_reason}` : ""}`,
                      ],
                    ]}
                  />
                  {entry.flow.depends_on ? <DependsOn dependsOn={entry.flow.depends_on} /> : null}
                </Section>
              ) : null}
              {entry.webhook ? (
                <Section title="Webhook">
                  <WebhookFacts webhook={entry.webhook} webhooksEnabled={webhooksEnabled} />
                </Section>
              ) : null}
              <Section title="Last execution">
                {entry.last_execution ? (
                  <p className="flex flex-wrap items-center gap-2 text-sm">
                    <StateChip state={entry.last_execution.state} kind="execution" />
                    <Link
                      className="font-mono text-xs hover:underline"
                      to={`${base}/executions/${encodeURIComponent(entry.last_execution.id)}/overview`}
                    >
                      {entry.last_execution.id.slice(0, 8)}
                    </Link>
                    <span className="text-muted-foreground" title={fullTimestamp(entry.last_execution.created_at)}>
                      {timeAgo(entry.last_execution.created_at)}
                    </span>
                  </p>
                ) : (
                  <p className="text-sm text-muted-foreground">This trigger has not started an execution.</p>
                )}
              </Section>
            </>
          ) : null}
        </div>
      </SheetContent>
    </Sheet>
  );
}
