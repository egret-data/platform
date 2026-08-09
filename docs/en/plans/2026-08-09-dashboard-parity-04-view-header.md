# W4 — View-Mode Header Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the dashboard page's view-mode header to Metabase's shape — an auto-refresh
popover with a countdown instead of a native `<select>`, a manual refresh that today does not
exist, a "…" menu that finally reaches the four dashboard-level mutations the backend has always
had, and the seven keyboard shortcuts that map onto features egret actually ships.

**Architecture:** Four independent additions that share one rule — **none of them may live in
`DashboardView.tsx`.** The refresh control (including its per-second tick) is one new component;
the "…" rows are a second export inside the component W3 created; the shortcuts are a hook plus
the null-rendering child the route already uses three times for exactly this purpose
(`ParamOptions`, `StaticOptions`, `CardEmptinessProbe`). The route contributes wiring and nothing
else. The one piece of shared state — "when did the last refresh happen" — goes into
`useDashboardViewControls`, which already owns everything else this header reads from the URL.

**Tech Stack:** React 18 + TypeScript, Radix Popover via the shadcn wrapper, Tailwind v4 (OKLCH
tokens), lucide-react, react-router-dom v6 `useSearchParams`, vitest + Testing Library +
user-event. No backend work, no migration, no new dependency.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §3.2 and §4 W4. §9
lists findings that did **not** survive inspection — in particular **the seven auto-refresh values
already match Metabase exactly**. Do not plan, write, or "fix" work that changes them.

**Branch:** `feat/dashboard-parity-04-view-header`, branched from
`feat/dashboard-parity-03-width-control` (**not** from `main`). W4 extends the component W3
creates. The PR body must not re-list W3's commits.

```bash
git fetch origin
git switch feat/dashboard-parity-03-width-control
git switch -c feat/dashboard-parity-04-view-header
```

---

## Background the implementer needs

**W3's menu is a Popover, and its rows are plain buttons.** W3's plan argues this at length and it
is not re-litigated here: `DashboardActionsMenu.view.tsx` hosts a `role="switch"`, a `role="switch"`
cannot nest inside a `role="menuitem"`, so the component is a Radix **Popover** and not a Radix
DropdownMenu. Consequence for W4: every row you add is a plain `<button type="button">`. A
`<DropdownMenuItem>` outside a `DropdownMenu.Root` throws at render for want of its context. That
also rules out reusing `MoveToFolderItems` (`components/explore/MoveToFolderSub.tsx`), which is
built entirely from `DropdownMenuItem`; the folder rows are reimplemented here with **identical
labels** so the two surfaces read the same even though they cannot share code.

**The list page does not confirm Duplicate, Move, or Move to Trash — so neither do we.** Read
`Dashboards.tsx:112-121`. Duplicate runs and navigates to the copy. Move applies immediately.
`onTrash` is `trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash"))` — no dialog, no
prompt. The **only** confirmed action on that page is "Delete permanently", which uses
`ConfirmDeleteDialog` (a type-the-word-`delete` gate) and is **not** on Metabase's dashboard-page
menu and therefore not on ours. So the four rows in this workstream carry exactly one dialog:
`FolderNameDialog` for "Move to new folder…", matching `Dashboards.tsx:259-272`. If you find
yourself writing a second confirm dialog, you are making the same action behave two ways
depending on which page it was invoked from — stop.

**Nothing bumps `refreshNonce` except the interval.** `useDashboardViewControls.ts:25-29` is the
whole refresh mechanism: a `setInterval` that increments a number used as a react-query key. There
is no manual path and no record of *when* the last refresh happened, which is precisely the value a
countdown needs. Both arrive together in Task 2, in one piece of state, so it is impossible to bump
the nonce and forget the origin.

**The countdown tick must not be route state — and that is a measured decision, not a style
preference.** `DashboardView` renders 22 card tiles on the harness dashboard, and its render body
recomputes `layout`, `cardQueryArgs(c)` and `applyParamsToChart(...)` per card on every pass
(`DashboardView.tsx:338-352, 610-612`), none of it memoised. A `useState` ticking once a second in
`useDashboardViewControls` — which is called at `DashboardView.tsx:84` — would run all of that 60
times a minute for a clock. Metabase already refuses to do this: `use-dashboard-refresh-period.ts`
keeps `elapsed` in a **ref** and pushes it through a callback the widget registered
(`setRefreshElapsedHook`), so the only component whose state changes each second is `RefreshWidget`
itself (`RefreshWidget.tsx:63,68-74`). We reach the same place more simply: the hook exposes
`refreshedAt` (an epoch stamp that changes **once per period**, at the same moment the nonce bumps
and the route was re-rendering anyway), and `RefreshMenu` owns its own `setInterval` + `useState`
and derives `remaining = period − (now − refreshedAt)`. The per-second re-render is confined to one
16px button. Task 6 measures this with a `MutationObserver` rather than asserting it.

**`resetParams` goes in `useDashboardViewControls`, not `useDashboardParams`.** Two reasons. The
first is mechanical: react-router hands a functional `setSearchParams` updater the search params of
the **current location**, which does not change between two synchronous calls, so a loop of
`paramsState.setValue(slug, null)` would have every call read the same `prev` and the last write
would win — every slug but one would survive the "reset". The reset has to be one write over all
slugs. The second is ownership: the index's file table gives `api/useDashboardParams.ts` to **W6
alone**, and the whole point of that table is that two workstreams do not grow one file.
`useDashboardViewControls.ts` is W4's column, already owns this route's query string (`?refresh`,
`?fullscreen`), and already calls `useSearchParams`.

**A parameter slugged `refresh` or `fullscreen` collides with the view controls.** `slugify` derives
a slug from the filter's name, so a filter named "Refresh" produces `?refresh=…` and is read as an
auto-refresh period. This is pre-existing (`useDashboardViewControls.ts:15-17` predates every
parameter feature) and is **out of scope** — do not fix it here and do not add a guard that makes
`resetParams` behave differently from `setValue`.

**The shortcuts hook cannot be called from the route body.** `DashboardView` returns early while the
dashboard loads (`:175`), and every handler a shortcut needs — `startEdit`, `cancelEdit`, `onSave`,
`addFilter`, `openPanel`, `tabs` — is defined *after* that return. Calling a hook there breaks the
Rules of Hooks. The file already solves this three times by rendering a null child that calls the
hook inside itself; the comment at `:43-44` says so in as many words. `useDashboardShortcuts.ts`
therefore exports both the hook and a `DashboardShortcuts` component. It needs no JSX (`return
null`), so the file stays `.ts` exactly as the index names it, and
`react-refresh/only-export-components` is `off` repo-wide (`eslint.config.js:31`) so the mixed
export is not a lint problem.

**Bare-letter shortcuts and text entry cannot share a keyboard.** The dashboard has an inline name
`<input>` (`DashboardView.tsx:471`), an inline heading `<input>` (`HeadingCard.tsx:17-27`) and an
inline text `<textarea>` (`TextCard.tsx:16-25`). Without a guard, typing "s" into a heading saves
the dashboard and typing "e" leaves edit mode mid-word. This is the single most likely way this
workstream ships a defect, so it gets its own test and its own mutation check.

**The detail DTO has no `folder_id`.** `DashboardDetailDto` (`crates/egret-analytics-api/src/bi.rs:360-372`)
carries id, name, description, parameters, created_by, updated_at, width, deleted_at, bookmarked,
tabs and cards — and nothing about folders. Only `DashboardSummary` has `folder_id`
(`api/dashboards.ts:40`). So "which folder is this dashboard in", which the rows need to disable the
current folder and to offer "Remove from folder", comes from the **list** query. `useDashboards(ws)`
is already in the react-query cache whenever the user arrived from the list page, and is a cheap
call when they did not. Adding `folder_id` to the DTO would be the better fix and is backend work
the index explicitly scopes out of W4.

**`refreshNonce` is a react-query key, not a signal.** `DashboardCardView` and `CardEmptinessProbe`
both take it (`DashboardView.tsx:571,639`). A manual refresh must bump the *same* number, so no
consumer has to learn the difference between a scheduled refresh and an asked-for one.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/RefreshMenu.tsx` | **New.** The seven-period table, the pure countdown maths (`secondsRemaining`, `formatRemaining`, `ringPercent`), the countdown ring, the "Refresh now" row and the popover. **Owns the per-second tick** — it exists here so it stops here. |
| `frontend/src/lib/useDashboardShortcuts.ts` | **New.** `isEditableTarget`, the keydown listener, and the null-rendering `DashboardShortcuts` child the route mounts. |
| `frontend/src/api/useDashboardViewControls.ts` | Gains `refreshedAt`, `refreshNow`, `resetParams`, `hasAnyParam`. The nonce and the countdown origin become one piece of state. |
| `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx` | W3's component gains optional controlled `open` / `onOpenChange`; the file gains a second export, `DashboardViewActionRows`, holding the view-mode rows and their folder dialog. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: one component swapped for another in the header, two components mounted, no new behaviour. Net budget **+10 lines**; more than that means an extraction was missed. |

Files this workstream must **not** touch:

- `components/chart/dashboard/DashboardActionsMenu.tsx` — the dashboard **list page** row menu. A
  different menu on a different surface. Read it for the house pattern; leave it alone.
- `components/chart/dashboard/DashboardEditToolbar.tsx` — W3 owns it. W4's menu is a **view-mode**
  instance rendered by the route; the edit-mode instance stays exactly as W3 left it.
- `api/useDashboardParams.ts` — W6 owns it (see the background note).
- `components/ui/popover.tsx` — no primitive needs changing.

---

## Task 1: The refresh popover, and the countdown that does not re-render the dashboard

**Files:**
- Create: `frontend/src/components/chart/dashboard/RefreshMenu.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/RefreshMenu.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/RefreshMenu.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  REFRESH_OPTIONS,
  RefreshMenu,
  formatRemaining,
  ringPercent,
  secondsRemaining,
} from "../RefreshMenu";

describe("countdown maths", () => {
  it("counts whole seconds down from the refresh origin and clamps at both ends", () => {
    // refreshedAt is an epoch stamp, `now` is another; the countdown is the difference. Clamping
    // matters because both ends are reachable in the wild: a tab that slept past the interval
    // wakes up with a negative remainder, and a clock that jumped backwards produces one larger
    // than the period.
    expect(secondsRemaining(300, 1_000_000, 1_000_000)).toBe(300);
    expect(secondsRemaining(300, 1_000_000, 1_010_000)).toBe(290);
    expect(secondsRemaining(300, 1_000_000, 1_999_999)).toBe(0);
    expect(secondsRemaining(300, 1_000_000, 900_000)).toBe(300);
  });

  it("formats as m:ss with the second zero-padded", () => {
    // Metabase's RefreshWidgetTarget.tsx:44-50. "4:9" would be read as four minutes nine seconds
    // by nobody.
    expect(formatRemaining(300)).toBe("5:00");
    expect(formatRemaining(249)).toBe("4:09");
    expect(formatRemaining(59)).toBe("0:59");
    expect(formatRemaining(0)).toBe("0:00");
  });

  it("caps the ring at 0.95 so a full period still reads as a countdown", () => {
    // RefreshWidgetTarget.tsx:53. A 100% ring and a 0% ring are the same circle, so without the
    // cap a period that has only just started looks like one that has just expired.
    expect(ringPercent(300, 300)).toBe(0.95);
    expect(ringPercent(150, 300)).toBe(0.5);
    expect(ringPercent(0, 300)).toBe(0);
  });
});

describe("RefreshMenu", () => {
  it("lists Metabase's seven periods in order with the active one checked", async () => {
    // The VALUES are not a divergence — design §9 records that both sides have shipped
    // Off/1/5/10/15/30/60 all along. This asserts the CONTROL: a checked row, by role and name.
    const user = userEvent.setup();
    render(<RefreshMenu refreshSeconds={300} onChangeRefreshSeconds={vi.fn()} refreshedAt={0} onRefreshNow={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    const rows = await screen.findAllByRole("menuitemradio");
    expect(rows.map((r) => r.textContent)).toEqual([
      "Off", "1 minute", "5 minutes", "10 minutes", "15 minutes", "30 minutes", "60 minutes",
    ]);
    expect(screen.getByRole("menuitemradio", { name: "5 minutes" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "Off" })).toHaveAttribute("aria-checked", "false");
    expect(REFRESH_OPTIONS.map((o) => o.seconds)).toEqual([null, 60, 300, 600, 900, 1800, 3600]);
  });

  it("reports the picked period in seconds, and null for Off", async () => {
    // Both directions, because a handler that always reports the row's label or always reports
    // null passes one of these and fails the other.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={onChange} refreshedAt={0} onRefreshNow={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "10 minutes" }));
    expect(onChange).toHaveBeenCalledWith(600);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Off" }));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("refreshes now without touching the period", async () => {
    // The manual path is the thing that did not exist: nothing but the interval ever bumped the
    // nonce (useDashboardViewControls.ts:27). Asking for one refresh must not schedule any.
    const user = userEvent.setup();
    const onRefreshNow = vi.fn();
    const onChange = vi.fn();
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={onChange} refreshedAt={0} onRefreshNow={onRefreshNow} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("button", { name: "Refresh now" }));
    expect(onRefreshNow).toHaveBeenCalledTimes(1);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("RefreshMenu countdown", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  it("shows the time to the next refresh and ticks it down", () => {
    vi.setSystemTime(1_000_000);
    render(<RefreshMenu refreshSeconds={300} onChangeRefreshSeconds={vi.fn()} refreshedAt={1_000_000} onRefreshNow={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Auto Refresh" });
    expect(trigger).toHaveAttribute("title", "Refreshing in 5:00");

    act(() => { vi.advanceTimersByTime(11_000); });
    expect(trigger).toHaveAttribute("title", "Refreshing in 4:49");
  });

  it("shows no countdown when auto-refresh is off, and starts no interval", () => {
    // The accessible NAME stays "Auto Refresh" whatever the clock says, so a test — and a screen
    // reader — can find the control without knowing the time.
    vi.setSystemTime(1_000_000);
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={vi.fn()} refreshedAt={1_000_000} onRefreshNow={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Auto Refresh" });
    expect(trigger).toHaveAttribute("title", "Auto-refresh");
    expect(vi.getTimerCount()).toBe(0);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/RefreshMenu.test.tsx`

Expected: FAIL — `Failed to resolve import "../RefreshMenu"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/RefreshMenu.tsx`:

```tsx
import { useEffect, useState } from "react";
import { Check, Clock, RotateCw } from "lucide-react";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";

/** The seven auto-refresh periods, in seconds.
 *
 *  These are IDENTICAL to Metabase's (RefreshWidget.tsx:12-55) and identical to the native
 *  <select> this popover replaces (DashboardView.tsx:485-495). Design §9 records the values as a
 *  finding that did NOT survive inspection: only the CONTROL diverged. Do not "correct" them.
 *  The labels do move to Metabase's wording ("Off", "5 minutes" rather than "No refresh",
 *  "5 min") — that is the control, not the value. */
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
 *  every card's query args and injected chart on each pass (DashboardView.tsx:338-352, 610-612),
 *  none of it memoised, so a clock ticking in route state would run all of that 60 times a minute.
 *  Metabase makes the same split for the same reason: its elapsed counter is a ref pushed into the
 *  widget through a callback, never dashboard state (use-dashboard-refresh-period.ts:7,24-34,
 *  RefreshWidget.tsx:63-74). Here the route only ever hands down `refreshedAt`, which changes once
 *  per period at the same moment the refresh itself re-renders everything. */
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/RefreshMenu.test.tsx`

Expected: PASS, 7 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Five mutations. Run the same command after each and restore before the next. Score every run by
process **exit code**, never by matching stdout text — the `PASS (n) FAIL (m)` line is the rtk
wrapper's reformatting and raw vitest never prints it.

1. `return Math.min(0.95, remaining / period);` → `return remaining / period;`.
   Expected: FAIL on "caps the ring at 0.95".
2. `secondsRemaining` → `return period - Math.floor((now - refreshedAt) / 1000);` (clamps dropped).
   Expected: FAIL on "counts whole seconds down … clamps at both ends".
3. `formatRemaining` → `` `${Math.floor(remaining / 60)}:${remaining % 60}` `` (padding dropped).
   Expected: FAIL on "formats as m:ss".
4. `aria-checked={selected}` → `aria-checked={false}`.
   Expected: FAIL on "lists Metabase's seven periods … with the active one checked".
5. Remove the `if (refreshSeconds === null) return;` early exit in the effect.
   Expected: FAIL on "shows no countdown when auto-refresh is off, and starts no interval" —
   `vi.getTimerCount()` becomes 1. If this one **passes**, the test is asserting less than it looks
   like it is; say so rather than moving on.

This step is not optional. Six assertions in the chart programme passed against a stubbed
implementation; doing exactly this is the only thing that caught them.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/RefreshMenu.tsx \
        frontend/src/components/chart/dashboard/__tests__/RefreshMenu.test.tsx
git commit -m "feat(dashboard): auto-refresh popover with a countdown trigger and a manual refresh row"
```

---

## Task 2: A manual refresh path, a countdown origin, and a one-write filter reset

**Files:**
- Modify: `frontend/src/api/useDashboardViewControls.ts`
- Modify: `frontend/src/api/__tests__/useDashboardViewControls.test.tsx`

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/api/__tests__/useDashboardViewControls.test.tsx`, inside the existing
`describe("useDashboardViewControls", …)` block (its `beforeEach`/`afterEach` already install and
remove vitest's fake timers, which also fake `Date`):

```tsx
  it("a manual refresh bumps the nonce AND moves the countdown origin", () => {
    // Two things have to move together. A nonce without an origin leaves the countdown showing
    // time to a refresh that already happened; an origin without a nonce restarts the clock
    // without re-running a single card query.
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d") });
    const nonce = result.current.refreshNonce;
    const at = result.current.refreshedAt;
    act(() => { vi.advanceTimersByTime(5000); result.current.refreshNow(); });
    expect(result.current.refreshNonce).toBe(nonce + 1);
    expect(result.current.refreshedAt).toBe(at + 5000);
  });

  it("the interval refresh moves the origin too, so the ring restarts", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=1") });
    const at = result.current.refreshedAt;
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.refreshNonce).toBe(1);
    expect(result.current.refreshedAt).toBe(at + 1000);
  });

  it("resetParams drops every slug in ONE write and leaves the view controls alone", () => {
    // The one-write property is the whole reason this lives here. A loop of per-slug writes would
    // hand every call the same `prev` — react-router's functional updater reads the CURRENT
    // location, which does not change between two synchronous calls — so only the last slug would
    // actually be removed.
    const { result } = renderHook(() => useDashboardViewControls(), {
      wrapper: wrap("/d?refresh=60&fullscreen=true&region=EU&tier=gold"),
    });
    expect(result.current.hasAnyParam(["region", "tier"])).toBe(true);
    act(() => { result.current.resetParams(["region", "tier"]); });
    expect(result.current.hasAnyParam(["region", "tier"])).toBe(false);
    expect(result.current.refreshSeconds).toBe(60);
    expect(result.current.isFullscreen).toBe(true);
  });

  it("hasAnyParam is false when no listed slug carries a value", () => {
    // This drives whether "Reset all filters" is offered at all, mirroring Metabase's
    // getCanResetFilters (dashboard/selectors.ts:753-756).
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=60") });
    expect(result.current.hasAnyParam(["region", "tier"])).toBe(false);
    expect(result.current.hasAnyParam([])).toBe(false);
  });
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/api/__tests__/useDashboardViewControls.test.tsx`

Expected: FAIL — `result.current.refreshNow is not a function`, and TypeScript errors on
`refreshedAt`, `resetParams` and `hasAnyParam`, none of which exist on `DashboardViewControls`.

- [ ] **Step 3: Write the implementation**

Replace the whole of `frontend/src/api/useDashboardViewControls.ts` with:

```ts
import { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

export interface DashboardViewControls {
  refreshSeconds: number | null;
  setRefreshSeconds: (s: number | null) => void;
  isFullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  refreshNonce: number;
  /** Epoch ms of the last refresh, scheduled or asked for. The countdown in RefreshMenu measures
   *  from here. It changes at most once per period — at the same moment the nonce bumps and the
   *  whole route re-renders anyway — so exposing it costs no render the dashboard was not already
   *  paying for. The per-SECOND tick deliberately lives in RefreshMenu, not here: this hook is
   *  called from DashboardView, which re-renders 22 card tiles. */
  refreshedAt: number;
  /** Re-run every card now. Bumps the SAME nonce the interval bumps, so no consumer has to learn
   *  the difference between a scheduled refresh and an asked-for one. */
  refreshNow: () => void;
  /** Drop every listed parameter slug from the URL in ONE history write.
   *
   *  It has to be one write. react-router hands a functional updater the search params of the
   *  current location, and that does not change between two synchronous calls, so a loop of
   *  per-slug writes would have every call read the same `prev` and the last write would win —
   *  leaving every slug but one in place after a "reset all". */
  resetParams: (slugs: string[]) => void;
  /** Does any listed slug currently carry a value in the URL? Drives whether "Reset all filters"
   *  is offered, mirroring Metabase's getCanResetFilters (dashboard/selectors.ts:753-756). */
  hasAnyParam: (slugs: string[]) => boolean;
}

/** Dashboard view controls synced to the URL: ?refresh=<seconds> and ?fullscreen=true. */
export function useDashboardViewControls(): DashboardViewControls {
  const [params, setParams] = useSearchParams();
  const raw = params.get("refresh");
  const refreshSeconds = raw && /^\d+$/.test(raw) && Number(raw) > 0 ? Number(raw) : null;
  const isFullscreen = params.get("fullscreen") === "true";
  // Nonce and origin are ONE piece of state on purpose: every refresh is both "re-run the card
  // queries" (the nonce, which is a react-query key) and "restart the countdown" (the stamp).
  // Keeping them together makes it impossible to bump one and forget the other.
  const [refresh, setRefresh] = useState(() => ({ nonce: 0, at: Date.now() }));

  const setRefreshSeconds = (s: number | null) =>
    setParams((p) => { const n = new URLSearchParams(p); if (s !== null && s > 0) n.set("refresh", String(s)); else n.delete("refresh"); return n; }, { replace: true });
  const setFullscreen = (v: boolean) =>
    setParams((p) => { const n = new URLSearchParams(p); if (v) n.set("fullscreen", "true"); else n.delete("fullscreen"); return n; }, { replace: true });
  const resetParams = (slugs: string[]) =>
    setParams((p) => { const n = new URLSearchParams(p); for (const s of slugs) n.delete(s); return n; }, { replace: true });
  const hasAnyParam = (slugs: string[]) => slugs.some((s) => params.has(s));

  const refreshNow = useCallback(() => setRefresh((r) => ({ nonce: r.nonce + 1, at: Date.now() })), []);

  useEffect(() => {
    if (!refreshSeconds) return;
    // Restart the countdown origin when the period changes: a freshly chosen "5 minutes" must read
    // 5:00, not inherit however much of the period it replaced had already elapsed.
    setRefresh((r) => ({ nonce: r.nonce, at: Date.now() }));
    const t = setInterval(refreshNow, refreshSeconds * 1000);
    return () => clearInterval(t);
  }, [refreshSeconds, refreshNow]);

  return {
    refreshSeconds, setRefreshSeconds, isFullscreen, setFullscreen,
    refreshNonce: refresh.nonce, refreshedAt: refresh.at, refreshNow,
    resetParams, hasAnyParam,
  };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/api/__tests__/useDashboardViewControls.test.tsx`

Expected: PASS, 7 tests (the 3 that were there plus the 4 added).

- [ ] **Step 5: Prove the tests survive mutation**

Three mutations; restore after each.

1. `refreshNow` → `setRefresh((r) => ({ nonce: r.nonce + 1, at: r.at }))`.
   Expected: FAIL on "a manual refresh bumps the nonce AND moves the countdown origin".
2. `resetParams` → delete only `slugs[0]`.
   Expected: FAIL on "resetParams drops every slug in ONE write".
3. `hasAnyParam` → `() => true`.
   Expected: FAIL on "hasAnyParam is false when no listed slug carries a value".

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/useDashboardViewControls.ts \
        frontend/src/api/__tests__/useDashboardViewControls.test.tsx
git commit -m "feat(dashboard): manual refresh, a countdown origin, and a single-write filter reset"
```

---

## Task 3: The header swaps its native `<select>` for the popover

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (the import block, and `:483-496`)
- Test: `frontend/src/routes/explore/__tests__/DashboardView.refresh.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/explore/__tests__/DashboardView.refresh.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return {
    ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false } }),
  };
});

/** Renders the current search string so the URL sync can be asserted without reaching into the
 *  router's internals. */
function ShowSearch() {
  return <span data-testid="search">{useLocation().search}</span>;
}

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <ShowSearch />
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView auto-refresh control", () => {
  it("offers the popover, not a native select", () => {
    renderView();
    expect(screen.getByRole("button", { name: "Auto Refresh" })).toBeInTheDocument();
    // The <select> that was here carried aria-label="auto-refresh" (DashboardView.tsx:485).
    expect(screen.queryByRole("combobox", { name: "auto-refresh" })).toBeNull();
  });

  it("picking a period writes ?refresh=<seconds>, keeping egret's existing URL sync", () => {
    // The URL contract is unchanged by design — a bookmarked ?refresh=300 must keep working.
    const user = userEvent.setup();
    renderView();
    return (async () => {
      await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
      await user.click(await screen.findByRole("menuitemradio", { name: "5 minutes" }));
      expect(screen.getByTestId("search").textContent).toBe("?refresh=300");
    })();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.refresh.test.tsx`

Expected: FAIL — no button named "Auto Refresh"; the header still renders the native `<select>`.

- [ ] **Step 3: Write the implementation**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import beside the other dashboard
component imports (after the `DashboardEditToolbar` import at `:30`):

```tsx
import { RefreshMenu } from "@/components/chart/dashboard/RefreshMenu";
```

Replace the whole `<label>…</label>` block at `:483-496` — the `sr-only` span, the `<select>` and
its seven `<option>`s — with:

```tsx
              <RefreshMenu
                refreshSeconds={view.refreshSeconds}
                onChangeRefreshSeconds={view.setRefreshSeconds}
                refreshedAt={view.refreshedAt}
                onRefreshNow={view.refreshNow}
              />
```

That is a 14-line block out and a 6-line one in. Nothing else in the route changes: `view` already
carries all four values after Task 2, and `view.refreshNonce` is still what the tiles and the
emptiness probes consume (`:571`, `:639`).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.refresh.test.tsx`

Expected: PASS, 2 tests.

- [ ] **Step 5: Run every existing dashboard-route test**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/`

Expected: PASS. If anything fails looking for `aria-label="auto-refresh"`, that test was asserting
the old control — update it to the popover rather than keeping the `<select>` alive alongside it.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.refresh.test.tsx
git commit -m "feat(dashboard): replace the native auto-refresh select with the refresh popover"
```

---

## Task 4: The view-mode "…" rows reach the four mutations nothing reached

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx` (W3's file)
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (the header's non-editing branch)
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx`

Read "The list page does not confirm Duplicate, Move, or Move to Trash" and "W3's menu is a
Popover" in the background before starting.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx`:

```tsx
import type { ComponentProps } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DashboardViewActionRows } from "../DashboardActionsMenu.view";

const nav = vi.fn();
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => nav,
}));

const duplicate = { mutateAsync: vi.fn().mockResolvedValue({ id: "d2" }) };
const trash = { mutateAsync: vi.fn().mockResolvedValue(undefined) };
vi.mock("@/api/dashboards", () => ({
  useDashboardMutations: () => ({ duplicate, trash }),
  // The detail DTO has no folder_id (bi.rs:360-372), so the rows read it from the list query.
  useDashboards: () => ({ data: [{ id: "d1", folder_id: "f1" }] }),
}));

const setFolder = { mutateAsync: vi.fn().mockResolvedValue(undefined) };
const createFolder = { mutateAsync: vi.fn().mockResolvedValue({ id: "f9" }) };
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [{ id: "f1", name: "Ops" }, { id: "f2", name: "Finance" }] }),
  useFolderMutations: () => ({ create: createFolder }),
  useSetFolder: () => setFolder,
}));

const success = vi.fn();
vi.mock("sonner", () => ({ toast: { success, error: vi.fn() } }));

function renderRows(over: Partial<ComponentProps<typeof DashboardViewActionRows>> = {}) {
  return render(
    <MemoryRouter>
      <DashboardViewActionRows
        ws="demo" dashboardId="d1" canResetFilters onResetFilters={vi.fn()} {...over}
      />
    </MemoryRouter>,
  );
}

const openMenu = async (user: ReturnType<typeof userEvent.setup>) =>
  user.click(screen.getByRole("button", { name: "dashboard actions" }));

beforeEach(() => { vi.clearAllMocks(); });

describe("DashboardViewActionRows", () => {
  it("offers Metabase's view-mode actions, by role and name", async () => {
    // Metabase DashboardActionMenu.tsx:88-163, minus subscriptions (design §5, no backend),
    // fullscreen (already its own header button) and Edit settings (no sidebar to open).
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    for (const name of [
      "Reset all filters", 'Move to "Ops"', 'Move to "Finance"', "Remove from folder",
      "Move to new folder…", "Duplicate", "Move to trash",
    ]) {
      expect(await screen.findByRole("button", { name })).toBeInTheDocument();
    }
  });

  it("disables the folder the dashboard is already in", async () => {
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    expect(await screen.findByRole("button", { name: 'Move to "Ops"' })).toBeDisabled();
    expect(screen.getByRole("button", { name: 'Move to "Finance"' })).toBeEnabled();
  });

  it("Duplicate duplicates this dashboard and opens the copy", async () => {
    // Identical to the list page (Dashboards.tsx:114) — same mutation, same navigation.
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: "Duplicate" }));
    expect(duplicate.mutateAsync).toHaveBeenCalledWith("d1");
    expect(trash.mutateAsync).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(nav).toHaveBeenCalledWith("/explore/demo/dashboards/d2"));
  });

  it("Move to trash trashes immediately, with NO confirmation, and returns to the list", async () => {
    // The list page does not confirm this (Dashboards.tsx:117) and neither does this menu. The
    // same action must not behave two ways depending on where it was invoked from.
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: "Move to trash" }));
    expect(trash.mutateAsync).toHaveBeenCalledWith("d1");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await vi.waitFor(() => expect(success).toHaveBeenCalledWith("Moved to Trash"));
    expect(nav).toHaveBeenCalledWith("/explore/demo/dashboards");
  });

  it("moving to another folder sets it and closes the menu", async () => {
    const user = userEvent.setup();
    renderRows();
    await openMenu(user);
    await user.click(await screen.findByRole("button", { name: 'Move to "Finance"' }));
    expect(setFolder.mutateAsync).toHaveBeenCalledWith({ id: "d1", folder_id: "f2" });
    await vi.waitFor(() => expect(screen.queryByRole("button", { name: "Duplicate" })).toBeNull());
  });

  it("offers no reset row when there is nothing to reset", async () => {
    // Metabase hides it on the same condition (getCanResetFilters, selectors.ts:753-756).
    const user = userEvent.setup();
    renderRows({ canResetFilters: false });
    await openMenu(user);
    expect(await screen.findByRole("button", { name: "Duplicate" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset all filters" })).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx`

Expected: FAIL — `DashboardViewActionRows` is not exported from `../DashboardActionsMenu.view`.

- [ ] **Step 3: Make W3's popover optionally controlled**

In `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx`, add two fields to the
existing `Props` interface, directly above `triggerClassName`:

```tsx
  /** Controlled popover state. Optional, and both halves must be supplied together: the edit-mode
   *  toolbar leaves the popover uncontrolled (Radix treats `open === undefined` as uncontrolled,
   *  so W3's call site is byte-for-byte unaffected), while the view-mode rows control it because
   *  two of them have to CLOSE the menu before opening a dialog — a Radix dialog's overlay takes
   *  the outside-pointerdown that would otherwise dismiss the popover mid-interaction. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
```

Widen the destructuring and pass both through:

```tsx
export function DashboardViewActionsMenu({ width, onChangeWidth, children, triggerClassName, open, onOpenChange }: Props) {
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
```

- [ ] **Step 4: Add the rows to the same file**

Extend the import block at the top of `DashboardActionsMenu.view.tsx`:

```tsx
import { useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, Folder, FolderMinus, FolderPlus, MoreHorizontal, RotateCcw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { useDashboardMutations, useDashboards } from "@/api/dashboards";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
```

(The `import type { ReactNode }` and the `MoreHorizontal`-only lucide import W3 wrote are folded
into the two lines above; nothing else in W3's file moves.)

Append to the end of the file:

```tsx
/** Every row in this popover. Plain <button>s, because this is a Popover and not a DropdownMenu —
 *  a <DropdownMenuItem> here throws for want of a DropdownMenu.Root ancestor. */
const ROW =
  "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50";

interface RowsProps {
  ws: string;
  dashboardId: string;
  /** Whether any of the dashboard's parameters currently carries a value. Mirrors Metabase's
   *  getCanResetFilters (dashboard/selectors.ts:753-756): the row is absent when there is nothing
   *  to reset, rather than present and inert. */
  canResetFilters: boolean;
  onResetFilters: () => void;
}

/** The dashboard PAGE's view-mode "…" menu contents: Reset all filters, the folder rows,
 *  Duplicate, Move to trash. Metabase's DashboardActionMenu.tsx:88-163, minus subscriptions
 *  (declined in design §5 — egret has no such backend), Enter fullscreen (already its own header
 *  button, DashboardView.tsx:497-506) and Edit settings (egret has no dashboard-settings sidebar
 *  to open). Every mutation it calls has existed and been unreachable from this page since the
 *  dashboard route was written — design §3.2.
 *
 *  IT RENDERS THE MENU rather than being passed to it as children, because two rows have to close
 *  the popover before opening a dialog; hence the controlled `open`.
 *
 *  BEHAVIOUR IS COPIED FROM THE LIST PAGE (Dashboards.tsx:112-121) DELIBERATELY. Duplicate
 *  navigates to the copy; a folder move applies immediately; Move to trash trashes and toasts
 *  "Moved to Trash" with NO confirmation, because the list page does not confirm it either and one
 *  action must not behave two ways. The only thing the list page confirms is "Delete permanently",
 *  which is not on Metabase's dashboard-page menu and is not on this one. */
export function DashboardViewActionRows({ ws, dashboardId, canResetFilters, onResetFilters }: RowsProps) {
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const [newFolderOpen, setNewFolderOpen] = useState(false);
  const { duplicate, trash } = useDashboardMutations(ws);
  const { data: folders = [] } = useFolders(ws, "dashboards");
  const { create: createFolder } = useFolderMutations(ws, "dashboards");
  const setFolder = useSetFolder(ws, "dashboards");
  // The detail DTO carries no folder_id (bi.rs:360-372), so the folder this dashboard sits in has
  // to come from the list query — already in the react-query cache whenever the user arrived from
  // the list, and a cheap call when they did not.
  const { data: summaries } = useDashboards(ws);
  const currentFolderId = summaries?.find((d) => d.id === dashboardId)?.folder_id ?? null;

  /** Close the menu, then let the mutation run; surface its failure the way every other dashboard
   *  action does (`guard` in Dashboards.tsx:76). */
  const run = (p: Promise<unknown>) => { setOpen(false); p.catch((e) => toast.error(String(e))); };

  return (
    <>
      <DashboardViewActionsMenu open={open} onOpenChange={setOpen}>
        {canResetFilters && (
          <>
            <button type="button" className={ROW} onClick={() => { setOpen(false); onResetFilters(); }}>
              <RotateCcw className="h-4 w-4 shrink-0" aria-hidden /> Reset all filters
            </button>
            <div role="separator" className="my-1 h-px bg-border" />
          </>
        )}
        {folders.map((f) => (
          <button
            key={f.id}
            type="button"
            className={ROW}
            disabled={f.id === currentFolderId}
            onClick={() => run(setFolder.mutateAsync({ id: dashboardId, folder_id: f.id }).then(() => toast.success("Moved")))}
          >
            <Folder className="h-4 w-4 shrink-0" aria-hidden /> Move to &quot;{f.name}&quot;
          </button>
        ))}
        {currentFolderId !== null && (
          <button
            type="button"
            className={ROW}
            onClick={() => run(setFolder.mutateAsync({ id: dashboardId, folder_id: null }).then(() => toast.success("Moved")))}
          >
            <FolderMinus className="h-4 w-4 shrink-0" aria-hidden /> Remove from folder
          </button>
        )}
        <button type="button" className={ROW} onClick={() => { setOpen(false); setNewFolderOpen(true); }}>
          <FolderPlus className="h-4 w-4 shrink-0" aria-hidden /> Move to new folder&hellip;
        </button>
        <button
          type="button"
          className={ROW}
          onClick={() => run(duplicate.mutateAsync(dashboardId).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
        >
          <Copy className="h-4 w-4 shrink-0" aria-hidden /> Duplicate
        </button>
        <div role="separator" className="my-1 h-px bg-border" />
        <button
          type="button"
          className={ROW}
          onClick={() => run(trash.mutateAsync(dashboardId).then(() => {
            toast.success("Moved to Trash");
            // Leave the page: `trash`'s onSuccess invalidates only the LIST key
            // (api/dashboards.ts:129-132), so staying here would show a dashboard the server now
            // considers trashed with no banner to say so.
            nav(`/explore/${ws}/dashboards`);
          }))}
        >
          <Trash2 className="h-4 w-4 shrink-0" aria-hidden /> Move to trash
        </button>
      </DashboardViewActionsMenu>
      {/* Sibling of the popover, not a child: the popover is closed by the time this opens. Same
          dialog, same labels and same flow as the list page (Dashboards.tsx:259-272). */}
      <FolderNameDialog
        open={newFolderOpen}
        title="Move to new folder"
        submitLabel="Move"
        onOpenChange={setNewFolderOpen}
        onSubmit={(name) =>
          createFolder
            .mutateAsync(name)
            .then((f) => setFolder.mutateAsync({ id: dashboardId, folder_id: f.id }))
            .then(() => toast.success("Moved"))
            .catch((e) => toast.error(String(e)))
        }
      />
    </>
  );
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx`

Expected: PASS, 6 tests.

Then re-run W3's own test file, which must be untouched by the controlled-`open` change:

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx`

Expected: PASS, 5 tests. If the "renders extra rows, and renders no switch when no width is
supplied" test fails, `open` is being passed as `false` somewhere instead of left `undefined` and
the popover has become permanently closed.

- [ ] **Step 6: Wire the rows into the header**

In `frontend/src/routes/explore/DashboardView.tsx`, extend the existing
`DashboardActionsMenu.view` import (W3 added `dashboardWidthClass` from it):

```tsx
import { DashboardViewActionRows, dashboardWidthClass } from "@/components/chart/dashboard/DashboardActionsMenu.view";
```

Add the component as the last child of the `!editing` fragment in the header, directly after the
"dashboard info" button (`:507`):

```tsx
              <DashboardViewActionRows
                ws={ws}
                dashboardId={dashboard.id}
                canResetFilters={view.hasAnyParam(parameters.map((p) => p.slug))}
                onResetFilters={() => view.resetParams(parameters.map((p) => p.slug))}
              />
```

- [ ] **Step 7: Prove the tests survive mutation**

Four mutations; restore after each. Re-run the rows test file each time.

1. Swap the Duplicate row's `duplicate.mutateAsync` for `trash.mutateAsync`.
   Expected: FAIL on "Duplicate duplicates this dashboard and opens the copy".
2. `disabled={f.id === currentFolderId}` → `disabled={false}`.
   Expected: FAIL on "disables the folder the dashboard is already in".
3. `{canResetFilters && (` → `{true && (`.
   Expected: FAIL on "offers no reset row when there is nothing to reset".
4. Wrap the trash row in a confirm — set a `pendingTrash` state instead of calling the mutation.
   Expected: FAIL on "Move to trash trashes immediately, with NO confirmation". This one exists to
   stop a future reviewer from "improving" the row into a behaviour the list page does not have.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): reach duplicate, move, trash and reset-filters from the dashboard page itself"
```

---

## Task 5: Keyboard shortcuts that never fire while the user is typing

**Files:**
- Create: `frontend/src/lib/useDashboardShortcuts.ts`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/lib/__tests__/useDashboardShortcuts.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/lib/__tests__/useDashboardShortcuts.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DashboardShortcuts, isEditableTarget, type DashboardShortcutHandlers } from "../useDashboardShortcuts";

function handlers(over: Partial<DashboardShortcutHandlers> = {}): DashboardShortcutHandlers {
  return {
    editing: false,
    tabCount: 3,
    onEdit: vi.fn(), onCancelEdit: vi.fn(), onSave: vi.fn(), onToggleBookmark: vi.fn(),
    onToggleInfo: vi.fn(), onAddFilter: vi.fn(), onAddCard: vi.fn(), onSelectTab: vi.fn(),
    ...over,
  };
}

/** The hook plus the two editable surfaces the dashboard actually has: the inline name input
 *  (DashboardView.tsx:471) and the inline text-card textarea (TextCard.tsx:16-25). */
function Harness({ h }: { h: DashboardShortcutHandlers }) {
  return (
    <div>
      <DashboardShortcuts {...h} />
      <input aria-label="dashboard name" defaultValue="" />
      <textarea aria-label="text body" defaultValue="" />
    </div>
  );
}

describe("isEditableTarget", () => {
  it("recognises every surface the browser lets the user type into", () => {
    const input = document.createElement("input");
    const textarea = document.createElement("textarea");
    const button = document.createElement("button");
    const div = document.createElement("div");
    // jsdom does not derive isContentEditable from the attribute, so define it directly — the
    // property is what the guard reads in a real browser too.
    Object.defineProperty(div, "isContentEditable", { value: true });
    expect(isEditableTarget(input)).toBe(true);
    expect(isEditableTarget(textarea)).toBe(true);
    expect(isEditableTarget(div)).toBe(true);
    expect(isEditableTarget(button)).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
  });
});

describe("dashboard shortcuts while viewing", () => {
  it("e edits, o bookmarks, ] toggles the info sidebar", async () => {
    const user = userEvent.setup();
    const h = handlers();
    render(<Harness h={h} />);
    await user.keyboard("e");
    await user.keyboard("o");
    await user.keyboard("]");
    expect(h.onEdit).toHaveBeenCalledTimes(1);
    expect(h.onCancelEdit).not.toHaveBeenCalled();
    expect(h.onToggleBookmark).toHaveBeenCalledTimes(1);
    expect(h.onToggleInfo).toHaveBeenCalledTimes(1);
  });

  it("the edit-only keys do nothing while viewing", async () => {
    const user = userEvent.setup();
    const h = handlers();
    render(<Harness h={h} />);
    await user.keyboard("sfa");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onAddFilter).not.toHaveBeenCalled();
    expect(h.onAddCard).not.toHaveBeenCalled();
  });
});

describe("dashboard shortcuts while editing", () => {
  it("e cancels, s saves, f adds a filter, a adds a card", async () => {
    // `e` is the one key that means two things. A handler wired to onEdit unconditionally passes
    // the viewing test above and fails here, which is why both exist.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("e");
    await user.keyboard("s");
    await user.keyboard("f");
    await user.keyboard("a");
    expect(h.onCancelEdit).toHaveBeenCalledTimes(1);
    expect(h.onEdit).not.toHaveBeenCalled();
    expect(h.onSave).toHaveBeenCalledTimes(1);
    expect(h.onAddFilter).toHaveBeenCalledTimes(1);
    expect(h.onAddCard).toHaveBeenCalledTimes(1);
  });

  it("the view-only keys do nothing while editing", async () => {
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("o");
    await user.keyboard("]");
    expect(h.onToggleBookmark).not.toHaveBeenCalled();
    expect(h.onToggleInfo).not.toHaveBeenCalled();
  });
});

describe("dashboard tab shortcuts", () => {
  it("1-9 select a tab that exists and do nothing for one that does not", async () => {
    const user = userEvent.setup();
    const h = handlers({ tabCount: 3 });
    render(<Harness h={h} />);
    await user.keyboard("1");
    await user.keyboard("3");
    await user.keyboard("4");
    await user.keyboard("9");
    expect(h.onSelectTab).toHaveBeenCalledTimes(2);
    expect(h.onSelectTab).toHaveBeenNthCalledWith(1, 0);
    expect(h.onSelectTab).toHaveBeenNthCalledWith(2, 2);
  });
});

describe("shortcuts and text entry", () => {
  it("never fire while focus is in a text input or a textarea", async () => {
    // THE defect this guard exists to prevent: the dashboard has an inline name input and inline
    // heading/text card editors, so without it typing "s" in a heading saves the dashboard and
    // typing "e" leaves edit mode mid-word.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);

    const name = screen.getByLabelText("dashboard name");
    await user.click(name);
    await user.keyboard("sales east");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onAddFilter).not.toHaveBeenCalled();
    expect(h.onCancelEdit).not.toHaveBeenCalled();
    expect(name).toHaveValue("sales east");

    const body = screen.getByLabelText("text body");
    await user.click(body);
    await user.keyboard("a few notes");
    expect(h.onAddCard).not.toHaveBeenCalled();
    expect(body).toHaveValue("a few notes");

    // …and the very same keys still work once focus leaves the text.
    await user.click(document.body);
    await user.keyboard("s");
    expect(h.onSave).toHaveBeenCalledTimes(1);
  });

  it("leaves modifier chords to the browser", async () => {
    // Cmd/Ctrl+S must stay the browser's Save Page, not ours.
    const user = userEvent.setup();
    const h = handlers({ editing: true });
    render(<Harness h={h} />);
    await user.keyboard("{Meta>}s{/Meta}");
    await user.keyboard("{Control>}e{/Control}");
    expect(h.onSave).not.toHaveBeenCalled();
    expect(h.onCancelEdit).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/lib/__tests__/useDashboardShortcuts.test.tsx`

Expected: FAIL — `Failed to resolve import "../useDashboardShortcuts"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/lib/useDashboardShortcuts.ts`:

```ts
import { useEffect, useRef } from "react";

/** Is this event's target somewhere the user is typing?
 *
 *  Bare-letter shortcuts and text entry cannot share a keyboard. The dashboard has an inline name
 *  <input> (DashboardView.tsx:471), an inline heading <input> (HeadingCard.tsx:17-27) and an
 *  inline text <textarea> (TextCard.tsx:16-25); without this guard, typing "s" in a heading saves
 *  the dashboard and typing "e" leaves edit mode mid-word.
 *
 *  contentEditable is checked as well. Nothing on the dashboard uses it today, but it is the third
 *  way a browser says "the user is typing here", it costs one property read, and a card kind that
 *  grows a rich-text editor would otherwise reintroduce the bug silently. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return target.isContentEditable;
}

export interface DashboardShortcutHandlers {
  editing: boolean;
  /** How many tabs the dashboard has; `1`-`9` select one only when it exists. */
  tabCount: number;
  onEdit: () => void;
  onCancelEdit: () => void;
  onSave: () => void;
  onToggleBookmark: () => void;
  onToggleInfo: () => void;
  onAddFilter: () => void;
  onAddCard: () => void;
  onSelectTab: (pos: number) => void;
}

/** Metabase's dashboard shortcuts (palette/shortcuts/dashboard.ts), ported for the features egret
 *  has: `e` edit / cancel edit, `s` save, `o` bookmark, `]` info sidebar, `f` add filter,
 *  `a` add card, `1`-`9` select tab. Each keeps Metabase's mode — "When viewing" or
 *  "When editing" — so a key never means two things in one mode.
 *
 *  DELIBERATELY NOT PORTED:
 *
 *  - `q` and `n` ("Add query builder question" / "Add native question"). Both open Metabase's
 *    new-question-from-a-dashboard flow, which builds a question, saves it and drops it onto the
 *    board. egret has no such flow: a card is added by picking an ALREADY-SAVED chart
 *    (DashboardView.tsx:251-260, ChartPickerList). A key that opened the chart picker under the
 *    name "add native question" would be a different feature wearing Metabase's label, so `a`
 *    covers adding a card and there is nothing for `q`/`n` to do.
 *  - `$mod+backspace` ("Send dashboard to trash"). W4 gives the dashboard page a Move-to-trash
 *    action for the first time, and — matching the list page — it does not confirm. Unconfirmed is
 *    acceptable behind two deliberate clicks through a menu; behind one chord, with no undo
 *    anywhere in egret's dashboard surface, it is not. Design §4 W4 says the same. */
export function useDashboardShortcuts(handlers: DashboardShortcutHandlers): void {
  // The listener is registered ONCE and reads the current handlers through a ref, because the
  // route re-renders often (a card query landing, a filter changing) and rebinding a window
  // listener each time is pure cost. The ref is written in an effect rather than during render so
  // nothing observes a half-committed render.
  const ref = useRef(handlers);
  useEffect(() => { ref.current = handlers; });

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // Chords belong to the browser and the OS: Cmd+S saves the page, not the dashboard.
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      // An IME is mid-composition; the keystroke belongs to the text being composed.
      if (e.isComposing) return;
      if (isEditableTarget(e.target)) return;
      const h = ref.current;

      if (e.key >= "1" && e.key <= "9") {
        const pos = Number(e.key) - 1;
        if (pos < h.tabCount) { e.preventDefault(); h.onSelectTab(pos); }
        return;
      }

      switch (e.key) {
        case "e":
          // The one key that means two things — Metabase gives it both "dashboard-edit" (when
          // viewing) and "dashboard-cancel-edit" (when editing).
          e.preventDefault();
          if (h.editing) h.onCancelEdit(); else h.onEdit();
          return;
        case "s":
          if (!h.editing) return;
          e.preventDefault(); h.onSave(); return;
        case "f":
          if (!h.editing) return;
          e.preventDefault(); h.onAddFilter(); return;
        case "a":
          if (!h.editing) return;
          e.preventDefault(); h.onAddCard(); return;
        case "o":
          if (h.editing) return;
          e.preventDefault(); h.onToggleBookmark(); return;
        case "]":
          if (h.editing) return;
          e.preventDefault(); h.onToggleInfo(); return;
        default:
          return;
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}

/** Null-rendering child that registers the shortcuts.
 *
 *  DashboardView returns early while the dashboard is loading (DashboardView.tsx:175) and every
 *  handler above is defined AFTER that return, so the hook cannot be called from the route body
 *  without breaking the Rules of Hooks. Rendering a child that calls it is the pattern the route
 *  already uses for exactly this reason — see `ParamOptions` and `StaticOptions`
 *  (DashboardView.tsx:43-55) and `CardEmptinessProbe`. No JSX is needed, so this file stays a
 *  `.ts` as the plan index names it. */
export function DashboardShortcuts(props: DashboardShortcutHandlers): null {
  useDashboardShortcuts(props);
  return null;
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/lib/__tests__/useDashboardShortcuts.test.tsx`

Expected: PASS, 8 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Four mutations; restore after each.

1. Delete `if (isEditableTarget(e.target)) return;`.
   Expected: FAIL on "never fire while focus is in a text input or a textarea". **If it passes, the
   test is not driving focus** — check that `user.click(name)` actually focused the input before
   reporting anything else.
2. `if (h.editing) h.onCancelEdit(); else h.onEdit();` → `h.onEdit();`.
   Expected: FAIL on "e cancels, s saves, f adds a filter, a adds a card".
3. `if (pos < h.tabCount)` → unconditional.
   Expected: FAIL on "1-9 select a tab that exists and do nothing for one that does not".
4. Delete `if (e.ctrlKey || e.metaKey || e.altKey) return;`.
   Expected: FAIL on "leaves modifier chords to the browser".

- [ ] **Step 6: Wire the shortcuts into the route**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import beside the other `lib` import
(`useUnsavedChangesWarning` at `:37`):

```tsx
import { DashboardShortcuts } from "@/lib/useDashboardShortcuts";
```

Mount it as the first child of the outermost `<div>` in the returned tree (immediately after the
line at `:452`):

```tsx
      <DashboardShortcuts
        editing={editing}
        tabCount={tabs.length}
        onEdit={startEdit}
        onCancelEdit={cancelEdit}
        onSave={onSave}
        onToggleBookmark={() => { (dash.bookmarked ? removeBookmark : addBookmark).mutateAsync(dash.id).catch((e) => toast.error(String(e))); }}
        onToggleInfo={() => setShowInfo((v) => !v)}
        onAddFilter={addFilter}
        onAddCard={() => openPanel({ kind: "add" })}
        onSelectTab={setActiveTabPos}
      />
```

Every handler is an existing route function; nothing new is defined. Note that `onCancelEdit` is
`cancelEdit`, so once W2 lands its discard confirmation the `e` shortcut inherits it for free —
W4 must not add a second confirmation path of its own.

- [ ] **Step 7: Run the whole frontend gate set**

Each as its own command. Do not chain them, and do not pipe them into `rtk` — a swallowed pipe
breaks the `&&` behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all three green. Record the total test count from the third. The delta against the
`feat/dashboard-parity-03-width-control` branch's count is **+27**:

| File | New tests |
|---|---|
| `components/chart/dashboard/__tests__/RefreshMenu.test.tsx` | 7 |
| `api/__tests__/useDashboardViewControls.test.tsx` | 4 (3 → 7) |
| `routes/explore/__tests__/DashboardView.refresh.test.tsx` | 2 |
| `components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx` | 6 |
| `lib/__tests__/useDashboardShortcuts.test.tsx` | 8 |

Any other delta means a gate did not run or a file was not collected — investigate before opening
the PR rather than adjusting the expectation.

- [ ] **Step 8: Check the route did not grow**

```bash
git diff --stat feat/dashboard-parity-03-width-control -- frontend/src/routes/explore/DashboardView.tsx
```

Expected: roughly `+22 −15` — the shortcuts child (+11), the rows (+6), `RefreshMenu` (+6) and two
import lines, against the fourteen-line `<select>` block removed. A net larger than **+10** means
behaviour is living in the route that should be in one of the three new surfaces; find it before
committing.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/lib/useDashboardShortcuts.ts \
        frontend/src/lib/__tests__/useDashboardShortcuts.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): keyboard shortcuts for the dashboard route, inert while the user is typing"
```

---

## Task 6: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers that go in the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. A
screenshot that "looks right" is not evidence, and this workstream has one claim — "the countdown
does not re-render the dashboard" — that **cannot** be seen at all. It is measured here.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA served at `http://localhost:30080`.

- [ ] **Step 2: Read the refresh popover by role and name**

Open `http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
(admin@egret.local / Admin12345!) at 1440×900, click the clock button in the header, and evaluate:

```js
(() => {
  const rows = [...document.querySelectorAll('[role="menuitemradio"]')];
  return {
    manual: !![...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Refresh now'),
    heading: [...document.querySelectorAll('div')].some(d => d.textContent.trim() === 'Auto Refresh'),
    options: rows.map(r => r.textContent.trim()),
    checked: rows.filter(r => r.getAttribute('aria-checked') === 'true').map(r => r.textContent.trim()),
  };
})()
```

Required, exactly: `manual: true`, `heading: true`,
`options: ["Off","1 minute","5 minutes","10 minutes","15 minutes","30 minutes","60 minutes"]`,
`checked: ["Off"]`. Compare against Metabase's at `localhost:3033/dashboard/2` — same seven
strings, same order. Any difference in the **values** is a regression, not a finding: design §9
records that they already matched.

- [ ] **Step 3: Measure that the countdown ticks and the dashboard does not**

This is the number the whole design decision rests on. Pick **1 minute**, wait for the popover to
close, then evaluate:

```js
await (async () => {
  const grid = document.querySelector('.react-grid-layout');
  const btn = document.querySelector('button[aria-label="Auto Refresh"]');
  let gridMuts = 0, btnMuts = 0;
  const g = new MutationObserver(m => { gridMuts += m.length; });
  const b = new MutationObserver(m => { btnMuts += m.length; });
  const opts = { subtree: true, childList: true, attributes: true, characterData: true };
  g.observe(grid, opts); b.observe(btn, opts);
  const t0 = btn.getAttribute('title');
  await new Promise(r => setTimeout(r, 10000));
  g.disconnect(); b.disconnect();
  return { t0, t1: btn.getAttribute('title'), gridMuts, btnMuts, cards: grid.children.length };
})()
```

Required:

- `t0` and `t1` are both `Refreshing in 0:xx`, and `t1` is **10 ± 1 seconds** below `t0`.
- `btnMuts >= 10` — the trigger really is repainting once a second.
- **`gridMuts === 0`** — and this is the assertion. `cards` should read 22. A non-zero `gridMuts`
  during a period with no refresh in it means the tick is re-rendering the card tiles, i.e. the
  countdown state ended up in the route after all. Re-read `RefreshMenu`'s tick comment.

Record all five numbers.

- [ ] **Step 4: Measure the manual refresh**

Set auto-refresh back to **Off**, then evaluate the baseline, click **Refresh now**, and evaluate
again:

```js
performance.getEntriesByType('resource').filter(e => e.name.includes('/card-data')).length
```

Required: the count rises by the number of visible chart cards on the tab (22 on the harness, minus
any non-chart cards). A delta of 0 means `refreshNow` is not reaching `refreshNonce`.

- [ ] **Step 5: Read the "…" menu by role and name**

Click the **…** button and evaluate:

```js
[...document.querySelectorAll('[role="dialog"] button, [data-radix-popper-content-wrapper] button')]
  .map(b => ({ name: b.textContent.trim(), disabled: b.disabled }))
```

Required: the rows are, in order, `Reset all filters` (present only if a filter currently has a
value), the folder rows, `Move to new folder…`, `Duplicate`, `Move to trash`. Compare with
Metabase's menu on `localhost:3033/dashboard/2` and record both lists side by side; the only
expected differences are the three items design §5 and §4 W4 exclude — Subscriptions, Enter
fullscreen (egret has it as its own header button) and Edit settings.

- [ ] **Step 6: Exercise Duplicate and Move to trash on a COPY, never on the harness**

The harness is 22 cards per side on byte-identical data (`docs/en/guides/chart-parity-harness.md`).
Do not trash it and do not touch its cards.

1. From the harness dashboard's "…" menu, click **Duplicate**. Required: the browser lands on
   `/explore/demo/dashboards/<new-id>` and the copy shows 22 cards. Record the new id.
2. On the **copy**, open "…" and click **Move to trash**. Required: no dialog appears, a
   "Moved to Trash" toast appears, and the browser lands on `/explore/demo/dashboards`. Record that
   no confirmation was shown — this is the parity-with-the-list-page claim.
3. Open the Trash view on the list page and **Delete permanently** the copy, so the workspace is
   left as you found it.

- [ ] **Step 7: Prove the shortcuts do not fire while typing**

The one defect this workstream is most likely to ship.

1. On the harness dashboard press **e**. Required: edit mode opens.
2. Click into a **heading card** and type `sales summary`. Required: the heading reads
   `sales summary`, edit mode is still open (the `s` did not save, the `a` did not open the add
   panel, the `e` did not cancel), and the network panel shows **no** `PUT` to
   `/api/ws/demo/dashboards/…`.
3. Click into the dashboard **name** input and type `Ops east`. Same requirement.
4. Click the empty canvas so nothing is focused, press **s**. Required: exactly one `PUT` goes out
   and the "Saved" toast appears.
5. Press **]**, then **o**. Required: the info sidebar toggles, and the bookmark star fills.
6. If the dashboard has tabs, press **2**. Required: the second tab activates.

Record the observed `PUT` count for steps 2-4: it must be `0, 0, 1`.

- [ ] **Step 8: Open the PR and stop**

Open `feat/dashboard-parity-04-view-header` against `feat/dashboard-parity-03-width-control` with,
quoted verbatim in the body:

- the seven option labels from Step 2 and Metabase's, side by side;
- Step 3's five numbers, with `gridMuts` called out — it is the evidence for the countdown design;
- the `/card-data` request delta from Step 4;
- the two menu listings from Step 5;
- the `0, 0, 1` PUT count from Step 7;
- the frontend test count and its **+27** delta against the W3 branch;
- a one-line note that this branch is stacked on W3 and its diff must be read against that branch,
  not `main`.

Then **stop**. The merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `DashboardView.tsx` gained wiring only — three components mounted, two imports, one block
      swapped. `git diff --stat` against the W3 branch shows a net of **+10 lines or fewer**.
- [ ] The seven refresh **values** are unchanged: `[null, 60, 300, 600, 900, 1800, 3600]`. Only the
      control and the labels moved. Design §9 says the values were never the divergence.
- [ ] `?refresh=<seconds>` still round-trips: a pasted `?refresh=300` selects "5 minutes" and
      picking "5 minutes" writes `?refresh=300`.
- [ ] The per-second tick is inside `RefreshMenu` and nowhere else. `refreshedAt` changes at most
      once per period. Step 3's `gridMuts === 0` is recorded.
- [ ] `refreshNow` bumps the **same** `refreshNonce` the interval bumps, so no card consumer had to
      change.
- [ ] Duplicate, Move to folder and Move to trash behave **exactly** as `Dashboards.tsx:112-121`
      does — same mutations, same toasts, and no confirmation on any of the three. The only dialog
      in the workstream is `FolderNameDialog`.
- [ ] Every row in either popover is a plain `<button type="button">`. No `DropdownMenuItem`
      anywhere in `DashboardActionsMenu.view.tsx`.
- [ ] `DashboardActionsMenu.tsx` (the list-page menu) and `DashboardEditToolbar.tsx` (W3's) are
      untouched — `git diff --stat feat/dashboard-parity-03-width-control` names neither.
- [ ] `api/useDashboardParams.ts` is untouched; the reset lives in `useDashboardViewControls.ts`,
      which is W4's file in the index table.
- [ ] `q`, `n` and `$mod+backspace` are **not** bound, and the reason for each is written in
      `useDashboardShortcuts.ts` rather than only in this plan.
- [ ] The editable-target guard covers `input`, `textarea`, `select` and `contentEditable`, and its
      test fails when the guard is removed.
- [ ] Shortcuts ignore ctrl/meta/alt chords, so `Cmd+S` still belongs to the browser.
- [ ] W3's `DashboardActionsMenu.view.test.tsx` still passes unchanged — the controlled `open` is
      optional and the edit-mode call site stays uncontrolled.
- [ ] No test asserts a display string where the code keys on a raw one. The period rows are
      matched by `aria-checked` and the option table by `seconds`, never by the label alone.
