import { useEffect, useState } from "react";
import { Check, Clock, RotateCw } from "lucide-react";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";

/** The seven auto-refresh periods, in seconds.
 *
 *  These are IDENTICAL to Metabase's (RefreshWidget.tsx:12-55) and identical to the native
 *  <select> this popover replaces. Design §9 records the values as a finding that did NOT survive
 *  inspection: only the CONTROL diverged. Do not "correct" them. The labels do move to Metabase's
 *  wording ("Off", "5 minutes" rather than "No refresh", "5 min") — that is the control, not the
 *  value. */
export const REFRESH_OPTIONS: ReadonlyArray<{ label: string; seconds: number | null }> = [
  { label: "Off", seconds: null },
  { label: "1 minute", seconds: 60 },
  { label: "5 minutes", seconds: 300 },
  { label: "10 minutes", seconds: 600 },
  { label: "15 minutes", seconds: 900 },
  { label: "30 minutes", seconds: 1800 },
  { label: "60 minutes", seconds: 3600 },
];

/** Whole seconds until the next automatic refresh, clamped to [0, period].
 *
 *  Both clamps are reachable: a backgrounded tab wakes up past its interval (negative remainder),
 *  and a system clock that steps backwards produces one larger than the period. */
export function secondsRemaining(period: number, refreshedAt: number, now: number): number {
  const elapsed = Math.floor((now - refreshedAt) / 1000);
  return Math.max(0, Math.min(period, period - elapsed));
}

/** "m:ss" — Metabase's RefreshWidgetTarget.tsx:44-50. */
export function formatRemaining(remaining: number): string {
  const s = remaining % 60;
  return `${Math.floor(remaining / 60)}:${s < 10 ? "0" : ""}${s}`;
}

/** How much of the countdown ring is filled, capped at 0.95 exactly as Metabase caps it
 *  (RefreshWidgetTarget.tsx:53). A full ring and an empty one are the same circle, so without the
 *  cap a period that has only just started reads as one that has just expired. */
export function ringPercent(remaining: number, period: number): number {
  return Math.min(0.95, remaining / period);
}

/** Ported from Metabase's CountdownIcon.tsx: one circle whose 32-wide stroke is drawn as a
 *  dash of `percent * 100` out of a 100-unit circumference, rotated so the arc starts at 12
 *  o'clock. Cheaper than an arc path and it inherits `currentColor` from the trigger. */
function CountdownRing({ percent }: { percent: number }) {
  return (
    <svg
      width={16}
      height={16}
      viewBox="0 0 32 32"
      aria-hidden
      style={{ transform: `rotate(-${percent * 360 + 90}deg)`, borderRadius: "50%" }}
    >
      <circle
        r="16" cx="16" cy="16"
        fill="currentColor" fillOpacity="0.5" stroke="currentColor" strokeWidth="32"
        strokeDasharray={`${percent * 100} 100`}
      />
    </svg>
  );
}

/** Plain buttons, not DropdownMenuItems: this is a Popover (see DashboardActionsMenu.view.tsx). */
const ROW =
  "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground";

interface Props {
  /** The active period in seconds, or null for off. Comes from ?refresh=<seconds>. */
  refreshSeconds: number | null;
  onChangeRefreshSeconds: (s: number | null) => void;
  /** Epoch ms of the last refresh — the countdown's origin. Changes once per period. */
  refreshedAt: number;
  onRefreshNow: () => void;
}

/** The dashboard's auto-refresh control: Metabase's RefreshWidget.tsx:75-113 — an icon trigger
 *  opening a popover headed "Auto Refresh", a checkmark on the active row, the same seven
 *  periods — plus egret's "Refresh now", which Metabase has no equivalent of and which this
 *  dashboard had no way to do at all.
 *
 *  THE PER-SECOND TICK LIVES HERE AND NOWHERE ELSE. The route renders 22 card tiles and rebuilds
 *  every card's query args and injected chart on each pass, none of it memoised, so a clock ticking
 *  in route state would run all of that 60 times a minute. Metabase makes the same split for the
 *  same reason: its elapsed counter is a ref pushed into the widget through a callback, never
 *  dashboard state (use-dashboard-refresh-period.ts:7,24-34, RefreshWidget.tsx:63-74). Here the
 *  route only ever hands down `refreshedAt`, which changes once per period at the same moment the
 *  refresh itself re-renders everything. */
export function RefreshMenu({ refreshSeconds, onChangeRefreshSeconds, refreshedAt, onRefreshNow }: Props) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    // No period, no clock: an idle dashboard must not hold a timer open.
    if (refreshSeconds === null) return;
    // Re-align on every period change and on every refresh, so the first tick after either is a
    // whole second rather than whatever was left of the previous one.
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [refreshSeconds, refreshedAt]);

  const remaining = refreshSeconds === null ? null : secondsRemaining(refreshSeconds, refreshedAt, now);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          // The accessible name is fixed while the title carries the clock, so the control keeps
          // one name for a screen reader and for every test that looks it up by role.
          aria-label="Auto Refresh"
          title={remaining === null ? "Auto-refresh" : `Refreshing in ${formatRemaining(remaining)}`}
          className="inline-flex size-8 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          {refreshSeconds === null || remaining === null
            ? <Clock className="h-4 w-4" aria-hidden />
            : <CountdownRing percent={ringPercent(remaining, refreshSeconds)} />}
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-52 p-2">
        <button type="button" className={ROW} onClick={() => { setOpen(false); onRefreshNow(); }}>
          <RotateCw className="h-4 w-4 shrink-0" aria-hidden /> Refresh now
        </button>
        <div role="separator" className="my-1 h-px bg-border" />
        <div className="mb-1 px-2 text-xs font-bold uppercase tracking-wide text-muted-foreground">
          Auto Refresh
        </div>
        <div role="menu" aria-label="Auto Refresh periods">
          {REFRESH_OPTIONS.map((o) => {
            const selected = o.seconds === refreshSeconds;
            return (
              <button
                key={o.label}
                type="button"
                role="menuitemradio"
                aria-checked={selected}
                className={ROW}
                onClick={() => { setOpen(false); onChangeRefreshSeconds(o.seconds); }}
              >
                {/* The check keeps its box when unselected, so the labels do not shift as the
                    active row moves — Metabase does the same with opacity. */}
                <Check className={`h-4 w-4 shrink-0 ${selected ? "text-primary" : "invisible"}`} aria-hidden />
                {o.label}
              </button>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}
