# Chart Redesign 04 — Gauge

**Status:** approved design, pending implementation plan
**Date:** 2026-07-29
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Previous units:** bar `1fe27573`, line `1ee0819e`, table `2638db51`
**Vietnamese reference:** `docs/vi/specs/2026-07-29-chart-redesign-04-gauge.md`

## Why gauge jumped the queue

Usage order put gauge thirteenth. The audit moved it to fourth, and reading the renderer in full
moved it again in seriousness. Both sides were verified on 2026-07-29.

## What a gauge is for

Saying whether one number is where it should be. Not what the number is — a scalar does that better
— but whether it is **good**. The coloured ranges are the entire reason to choose a gauge, and
everything below follows from that.

## The defect: the ranges are fixed, and their direction is a guess

`echarts/options/gauge.ts` draws three zones:

```ts
axisLine: { lineStyle: { width: 18, color: [[1 / 3, ZONE_LOW], [2 / 3, ZONE_MID], [1, ZONE_HIGH]] } },
```

Fixed at exact thirds of `[min, max]`. Fixed colours. And fixed **direction** — low is red, high is
green.

That last one is not a missing feature, it is a wrong answer. A gauge showing p99 latency, error
rate, cost or churn is painted green at exactly the end where the news is worst. The chart states
the opposite of the truth, confidently, with no way to correct it. A missing signal makes a reader
look elsewhere; a wrong one makes them act.

Everything else about the gauge is sound: the value, the auto-range at 2× when no max is given, the
number formatting, the aria description naming the value and its scale.

## The model: ranges first, scale derived

Metabase's `gauge.segments` is an array of `{ min, max, color, label }`, and `gauge.range` is
**computed** from it — the smallest min and the largest max. The ranges are the model; the scale
falls out. egret has it backwards: a range set by hand, then split into three equal thirds whatever
those thirds happen to mean.

egret adopts the Metabase model:

```
gauge.segments : Array<{ min: number; max: number; color: string; label?: string }>
```

and **`gauge.min` and `gauge.max` are removed**. Keeping them would mean two sources of truth for
one scale, disagreeing the moment either is edited — the same trap chart 02 removed from the line
settings.

`label` is worth carrying: a range called "Healthy" tells the reader what the colour means without a
legend, which is exactly what a single-number chart has no room for.

### What an unconfigured gauge does

Nothing is stored until the user configures something, so the default is derived at render time —
the rule the program adopted in chart 01, and the reason a chart saved months ago picks up a changed
default. With no segments, the builder produces today's three equal zones over today's auto-range,
so **an unconfigured gauge renders exactly as it does now**. The wrongness is not fixed by fiat; it
becomes fixable, and stays wrong only where nobody has said otherwise.

## Migration 0102

Only charts that stored an explicit `gauge.min` or `gauge.max` have anything to convert. They become
three segments at the thirds they are drawn at today, with today's colours, so their appearance is
preserved exactly. Charts with neither stored get **no** segments and fall through to the render-time
default — writing segments for them would freeze an auto-range that currently follows the data.

Both old keys are removed.

## `fieldDescriptor` gains an escape hatch

Gauge is one of the nine types declared through `fieldDescriptor`, which supports four scalar field
kinds — dimension, metric, number, boolean. A repeating list of `{min, max, color, label}` is none of
them.

The choice is between hand-writing a gauge descriptor, as cartesian and table have, or giving
`fieldDescriptor` a `custom` kind that renders a supplied component. **This spec takes the second.**
Hand-writing reopens, for one type, the code that PR3 of A2 deleted; and gauge is not the last type
to need a non-scalar control — progress needs a colour, waterfall needs three, trend needs a
comparison baseline. One escape hatch used four times is better than four bespoke descriptors.

The hatch is narrow on purpose: `custom` supplies a render function and nothing else. It is not a
second way to declare scalar fields.

## Deliberately not done

- **No segment count limit and no automatic colours.** A user adding a fourth range is doing
  something reasonable; guessing colours for them is how the current defect started.
- **No "invert" toggle.** With editable ranges, reversing the direction is what the ranges are for;
  a flag would be a second way to say the same thing.
- **The pointer, the auto-range and the number formatting are untouched.** They work.
- **Progress and waterfall keep their colours as they are.** They will use the same hatch in their
  own units.

## Live check

On the Demo Sales dataset:

1. An unconfigured gauge renders exactly as it does on `main` — same zones, same range.
2. A gauge saved before this change, with min and max set, looks identical after the migration.
3. Editing a range's bounds moves the colour boundary on the arc.
4. Setting the top range red and the bottom green — the less-is-better case — renders that way,
   which is the defect this unit exists to fix.
5. Naming a range shows the name.
6. Adding a fourth range works; deleting down to one works.
7. The same gauge as a small dashboard card.

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check, before/after
screenshots at both sizes. There is a migration, so the backend gate is in scope, and the backend
must be rolled before the frontend — chart 02 established why.
