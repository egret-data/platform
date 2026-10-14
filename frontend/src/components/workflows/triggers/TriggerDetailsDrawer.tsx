import { useEffect, useId, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import { Copy, Eye, EyeOff, SquarePen } from "lucide-react";
import type { DependsOnDto, ScheduleInfoDto, TriggerEntryDto, WebhookInfoDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { StateChip } from "@/components/workflows/StateChip";
import {
  kindLabel,
  missedLabel,
  statusText,
  webhookNote,
  webhookPath,
  type FlowContext,
} from "@/components/workflows/triggers/triggersModel";
import dayjs from "dayjs";
import timezone from "dayjs/plugin/timezone";
import utc from "dayjs/plugin/utc";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";
import { MASK, maskWebhookKey } from "@/components/workflows/triggers/triggerYaml";

dayjs.extend(utc);
dayjs.extend(timezone);

/** What the drawer says when `?trigger=` names an id the flow no longer declares (spec §5). */
export const TRIGGER_GONE = "This trigger no longer exists in the flow; reload.";

/**
 * `iso` in `timeZone`, in `fullTimestamp`'s format (dayjs, `YYYY-MM-DD HH:mm:ss`), or null
 * when the browser does not know that zone.
 */
function inZone(iso: string, timeZone: string): string | null {
  try {
    return dayjs(iso).tz(timeZone).format("YYYY-MM-DD HH:mm:ss");
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
// The server percent-encodes each segment (`entry.rs`, `encode_segment`): a `/` inside the key is
// `%2F`, so the last segment is the whole key.
const masked = (path: string) => path.replace(/[^/]+$/, MASK);

function WebhookFacts({
  webhook,
  webhooksEnabled,
  revealed,
  onRevealedChange,
  hasSecret,
}: {
  webhook: WebhookInfoDto;
  webhooksEnabled: boolean;
  /** One switch for the path here and the `key:` in the Definition block. */
  revealed: boolean;
  onRevealedChange: (revealed: boolean) => void;
  /** The Definition block holds the key: Reveal shows it there, even with no path. */
  hasSecret: boolean;
}) {
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
        <code className="break-all rounded bg-muted px-2 py-1 font-mono text-xs" data-testid="webhook-path">
          {revealed ? path : masked(path)}
        </code>
      ) : null}
      {hasSecret || path ? (
      <div className="flex items-center gap-2">
        {/* One signal for the state: the pressed state, under a label that does not change. */}
        <Button type="button" size="sm" variant="outline" aria-pressed={revealed} onClick={() => onRevealedChange(!revealed)}>
          {revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          Reveal
        </Button>
        {!webhook.key_error && path ? (
          <>
            <Button type="button" size="sm" variant="outline" onClick={() => void copy()}>
              <Copy aria-hidden />
              Copy
            </Button>
            <span aria-live="polite" className="text-xs text-muted-foreground">
              {copied ? "Copied" : ""}
            </span>
          </>
        ) : null}
      </div>
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
  pending = false,
  failure = null,
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
  /**
   * The caller is still reading the trigger (the global page, W7e Task 7): a skeleton, not
   * "no longer exists". The same drawer stays mounted when the answer lands, so it opens once.
   */
  pending?: boolean;
  /** The caller's read failed: this sentence instead of the details. */
  failure?: string | null;
}) {
  // The webhook's key stays hidden, in its path and in the YAML, until Reveal. The tab keys the
  // drawer by trigger id, so another trigger starts hidden again.
  const [revealed, setRevealed] = useState(false);
  const captionId = useId();
  const isWebhook = entry?.kind === "webhook";
  const hiding = block !== null && isWebhook && !revealed ? maskWebhookKey(block) : null;
  const shownBlock = hiding ? hiding.text : block;
  return (
    <Sheet open={open} onOpenChange={(o) => { if (!o) onClose(); }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>
            Trigger <span className="font-mono">{triggerId}</span>
          </SheetTitle>
          <SheetDescription>
            {failure ? (
              "Not loaded"
            ) : pending ? (
              "Reading this trigger's details."
            ) : entry ? (
              <>
                {kindLabel(entry.kind)} · <span className="font-mono">{entry.type}</span>
              </>
            ) : (
              TRIGGER_GONE
            )}
          </SheetDescription>
        </SheetHeader>
        <div className="grid min-h-0 flex-1 content-start gap-5 overflow-auto">
          {failure ? (
            <Alert variant="destructive">
              <AlertDescription>{failure}</AlertDescription>
            </Alert>
          ) : pending ? (
            <div className="grid gap-2" data-testid="trigger-drawer-loading">
              <Skeleton className="h-5 w-2/3" />
              <Skeleton className="h-24 w-full" />
              <Skeleton className="h-5 w-1/2" />
            </div>
          ) : entry ? (
            <>
              <p className="text-sm">
                <span className="text-muted-foreground">Status: </span>
                {statusText(entry, ctx)}
              </p>
              <Section title="Definition">
                {shownBlock !== null ? (
                  <figure className="grid gap-1" aria-labelledby={captionId}>
                    <figcaption id={captionId} className="sr-only">
                      YAML of {triggerId}
                    </figcaption>
                    <pre className="overflow-auto rounded bg-muted p-3 font-mono text-xs">{shownBlock}</pre>
                    {hiding ? (
                      <p className="text-xs text-muted-foreground">
                        {hiding.whole
                          ? "The whole block is hidden: its key could not be told apart from the rest. Reveal under Webhook shows it."
                          : "The key is hidden; Reveal under Webhook shows it."}
                      </p>
                    ) : null}
                  </figure>
                ) : (
                  <p className="text-sm text-muted-foreground">This trigger&apos;s YAML cannot be shown here; open it in the editor.</p>
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
                  <WebhookFacts
                    webhook={entry.webhook}
                    webhooksEnabled={webhooksEnabled}
                    revealed={revealed}
                    onRevealedChange={setRevealed}
                    hasSecret={block !== null}
                  />
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
