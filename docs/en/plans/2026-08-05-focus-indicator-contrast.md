# The focus indicator fails its contrast bar — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the keyboard focus indicator visible enough to meet its bar, in both themes, without
breaking the two different indicator patterns already in the codebase.

**Architecture:** This is a **design-token change**, not a component change. The intended diff is small
— `--ring` in `index.css`, and possibly the alpha on one or two utility patterns. Everything else in
this plan is measurement and blast-radius control, because the token reaches 41 files across the whole
product, not just the charts.

**Tech stack:** Tailwind v4 with OKLCH design tokens, shadcn/Radix, React 18.

---

## The bar, stated up front

**3:1, not 4.5:1.** This is a **non-text** focus indicator — WCAG 1.4.11 Non-text Contrast and 2.4.11
Focus Appearance. State that explicitly in the PR rather than quietly measuring against the text bar;
a previous unit in this programme got this wrong in the other direction and had to say so.

The comparison is against the **adjacent** colours — the component's own background and the page
behind it — not against an abstract white.

## What was measured, and by whom

An accessibility audit at `1d9b0d30` converted the OKLCH tokens to sRGB and composited alpha
numerically, deliberately avoiding the trap this programme has hit before: `getComputedStyle` returns
`oklch(...)` for egret's tokens, so **a naive rgb regex reads its three numbers as RGB and reports
garbage**. #412 independently re-derived the table and matched it to two decimals.

| | light | dark |
|---|---|---|
| `--ring` solid | **2.58:1 FAIL** | 4.08:1 pass |
| `ring-ring/50` | **1.54:1 FAIL** | **1.83:1 FAIL** |
| global `* { outline-ring/50 }` fallback | **1.54:1 FAIL** | **1.83:1 FAIL** |

Tokens: light `--ring: oklch(0.71 0.01 286.09)` (`index.css:114`), dark
`--ring: oklch(0.55 0.02 285.93)` (`:165`), exposed as `--color-ring` (`:208`). The global fallback is
`@layer base { * { @apply border-border outline-ring/50 } }` (`:245-247`).

**#412 also settled one open question with a browser:** Chrome's `outline-style: auto` **does** honour
an author `outline-color` — forced red on a focused element and screenshotted the red ring. So the
global fallback needs no special treatment.

## The complication the audit's table hides

**There are two shadcn generations mixed in this codebase**, and they produce different indicators:

- **older**: `focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2` — a solid
  2px ring separated from the control by a `ring-offset-background` gap. Example: `ui/tabs.tsx:29`,
  `ui/switch.tsx:11`, `ui/radio-group.tsx:28`.
- **newer**: `focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]` — a
  **two-part** indicator: a solid `--ring` border plus a soft 3px halo at half alpha. Example:
  `ui/badge.tsx:8`.

**Measuring the `/50` halo alone and calling it a failure is not the whole analysis** — in the newer
pattern the solid `border-ring` is what carries the contrast, and the halo is decoration. Both still
fail today (2.58:1 for the border), but the *fix* differs: raising `--ring` fixes the newer pattern's
border outright, while the older pattern's ring already uses solid `--ring` too. The `/50` halo may
not need its own change at all.

**Establish this before changing anything.** If the conclusion is "one token change fixes every
pattern", that is the best possible outcome and the diff is three lines.

> **CORRECTION, from Task 1 (this is what measuring actually found).** The paragraph above is right
> about `Input`, `Badge`, `Textarea` and `SelectTrigger` and **wrong about `Button`**, which is the
> control the rule matters most for. `focus-visible:border-ring` sets a border *colour*; it cannot
> paint anything unless the element already has a border *width*. Button's `default`, `ghost` and
> `link` variants have none — measured in Chrome, `border-top-width` on a focused header button is
> `0px` — so for those variants there is no solid half at all and **the `/50` halo is the entire
> indicator**, at 1.54:1 light / 1.83:1 dark.
>
> That cannot be fixed by darkening the token: a 50% composite tops out near 1.73:1 no matter how
> dark `--ring` gets, because it is being diluted half-and-half into the surface. So the answer to
> "one fix or two" is **two concerns, three lines**: darken the token *and* drop the alpha in the two
> places where an alpha-reduced ring is the only indicator — the global `* { outline-ring/… }`
> fallback and `Button`. The other four primitives keep their soft halo, because their solid border
> really does carry them once the token passes.

## Blast radius, counted

41 files: `ring-ring` in **33**, `border-ring` in **5**, `outline-ring` in **3** (plus 74
`focus-visible:ring` occurrences overall). By area:

| area | files |
|---|---|
| `components/ui` | 12 |
| `components/chart` (+ `settings`, `__tests__`) | 11 |
| `components/sqllab` | 4 |
| `routes/explore` (+ `settings`) | 2 |
| `routes/control-plane` (config, access) | 2 |
| `components/notebook` | 1 |

**Only about a quarter of this is charts.** This PR leaves the chart programme's territory, and the
reviewer should know that.

---

## Task 1: measure it yourself, in a browser, before changing anything

**Do not take the table above on trust.** It is second-hand twice over, and this is a token change
that reaches every focusable element in the product.

- [ ] **Step 1: Derive the numbers independently.** Paint each colour to a 1×1 canvas and read it
  back, or convert OKLCH→sRGB numerically. Say which method you used. **Do not regex `oklch()`.**

- [ ] **Step 2: Measure the indicator as RENDERED, per pattern, not per token.** For each of the two
  generations, focus a real control in a real browser and measure the actual pixels of the indicator
  against the actual adjacent pixels. The older pattern has a `ring-offset-background` gap between
  control and ring — say what the ring is actually adjacent to.

- [ ] **Step 3: Check `--ring` is not used for anything that is not a focus indicator.** If it is,
  darkening it changes something that is not focus, and that has to be reported.

- [ ] **Step 4: Report before proceeding.** State: the measured numbers, whether the two patterns need
  one fix or two, and what the minimal change is.

## Task 2: change the token

- [ ] **Step 1: Write the failing test.** A test can assert the token's computed contrast without a
  browser if you compute it in the test — `index.css` is parseable and the maths is deterministic.
  Pin **both themes** and pin the **3:1** bar with a comment saying why it is 3 and not 4.5. Watch it
  fail on light.

- [ ] **Step 2: Darken light `--ring`.** The audit's unverified estimate was roughly `oklch(0.63 …)`;
  derive the real value rather than adopting it. Keep the hue and chroma — this is a neutral token and
  a colour shift would be a visual change nobody asked for.

- [ ] **Step 3: Re-measure dark.** It passes at 4.08:1 today. If your change touches the dark value,
  it must still pass; if you leave dark alone, say so.

- [ ] **Step 4: Decide about the `/50` alpha**, informed by Task 1 Step 2. If the solid border carries
  the indicator, leaving the halo at half alpha is correct and should be stated. If not, raise it and
  say what that costs visually.

- [ ] **Step 5: Gates, commit.**

## Task 3: look at it

A contrast number is not a design review. This programme has a recorded lesson on exactly this:
**measuring numbers does not replace looking at the picture** — a scalar's caption passed every
numeric check while sitting 300px from the number it captioned, and only a screenshot showed it.

- [ ] **Step 1: Screenshot a focused control in both themes**, before and after, for both generations.
  Absolute paths into the session scratchpad.

- [ ] **Step 2: Check it does not now read as a border rather than a focus ring.** A darker ring on a
  light surface is more visible by construction; the risk is that it looks heavy or that it collides
  with `--border` on adjacent elements.

- [ ] **Step 3: Check a control on a NON-white surface** — a focused item inside a popover, a dialog,
  a dark card. The audit measured against the page background; the indicator has to work on the
  surfaces it actually lands on.

## Task 4: the sweep, and the PR

- [ ] **Step 1: Gates**, then build and **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js` — the scratchpad `serve.py` serves `index.html` with no
  cache headers, so the first reload after a rebuild keeps the OLD bundle.

- [ ] **Step 2: Tab through one screen in each affected area** — a chart settings panel, SQL Lab, a
  notebook, a control-plane page. Not every file; one representative per area, and say which you
  chose. The point is to catch an area where the token means something different.

- [ ] **Step 3: Open the PR. Do NOT merge.** The body must say plainly that this is a **product-wide**
  change with only a quarter of its files under `components/chart/`, and list the areas touched, so
  whoever reviews it knows it is not a chart PR.

---

---

## Outcome — what was measured, changed, and left alone

**Measured three independent ways** before anything changed: OKLCH→sRGB converted numerically
(Ottosson), the same colours painted to a 1×1 canvas in real Chrome and read back, and — the part
nobody had done — the **painted pixels of a real focus ring**, scanned out of screenshots of controls
tabbed to with actual key events. All three agree to 8-bit rounding, and they reproduce the audit's
table exactly. (The canvas pass also confirmed dark `--card` at relative luminance **0.0094**, the
value this programme has on record as the real one.)

**The indicator as rendered, per generation, light mode, outward from the control's edge:**

| generation | painted bands | verdict |
|---|---|---|
| older (`ring-2` + `ring-offset-2`) | card border `#e4e4e4` → **2px white gap** → **2px `#a1a1a8` ring** → white page | ring adjacent to white on *both* sides, **2.57:1 FAIL** |
| newer, bordered (`Input`) | white page → **1px `#a1a1a8` border** → 3px `#cfcfd3` **inset** halo → white field | border carries it, **2.57:1 FAIL**; halo never touches the page |
| newer, borderless (`Button`) | white page → 3px `#cfcfd3` halo → white fill (no border at all) | **halo is the whole indicator, 1.54:1 FAIL** |
| no focus style of its own | Chrome `outline-style: auto` in `--ring/50` | **1.54:1 FAIL**, and this is the majority case |

That last row is the one the blast-radius count understates. Tabbing the control-plane pages, **11 of
the first 14 stops** — the entire left navigation — have no focus style of their own and land on the
global fallback.

**The change (three functional lines):**

1. light `--ring` `oklch(0.71 …)` → `oklch(0.63 …)`. 0.63 is not adopted from the audit's estimate,
   it is the first lightness clearing 3:1 on **every** surface a focused control lands on:
   background/card 3.51, popover 3.31, accent 3.21, dashboard 3.07, **secondary 3.03**. Measuring
   only against `--background` would have accepted ~0.67, which fails on `--secondary` at 2.91.
2. global fallback `outline-ring/50` → `outline-ring`.
3. `Button` `focus-visible:ring-ring/50` → `focus-visible:ring-ring`.

**Dark `--ring` was not touched** — it already passes (4.08 on background, 3.09 worst surface), and
the rendered dark pixels are byte-identical before and after.

**After, at the pixel level:** the older pattern's ring went `#a1a1a8` → `#88888f` (2.57 → **3.52:1**);
the global fallback's core outline pixel is now `#88888f` in light (**3.52:1**, was 1.54) and
`#70707d` in dark (**3.62:1**, was 1.83). Across 88 tab stops in four areas, every stop paints the
ring token and none is left on a half-alpha indicator.

**Did it start reading as a border?** No, and the numbers go the right way: `--ring` against
`--border` improved from 2.04:1 to **2.77:1**, so the focus ring is now *more* distinguishable from a
resting border than it was, not less. The older pattern also keeps its offset gap, which preserves
the "ring" reading rather than a thickened edge.

## Found while measuring, deliberately NOT fixed here

Each of these is a real defect; none is a contrast-token problem, and folding any of them in would
make this PR unreviewable.

- **Focused menu items show almost nothing.** A dropdown item on `--popover` does not use `--ring` at
  all — it indicates focus with an `--accent` background, measured at **1.03:1** against the popover
  (`#f5f5f5` on `#f8f8f8`). Keyboard users effectively cannot see where they are in a menu. This is
  an `--accent`/`--popover` problem and needs its own decision.
- **`ring-offset-color` defaults to `#fff`** wherever a call site uses `ring-offset-2` without
  `ring-offset-background` (e.g. `AccessOverview.tsx:376`). In dark mode that paints a **2px pure
  white band** inside the ring. Accidentally makes the ring *more* visible; still clearly unintended.
- **`resizable.tsx:40` has `focus-visible:ring-ring` with no ring width**, so it sets a colour and
  paints no ring at all.
- **`SqlLab.tsx:35` is the only non-focus use of `--ring`** (`ring-1 ring-ring`, no `focus-visible:`).
  It survived the Task 1 Step 3 check: the input is mounted only while renaming and unmounts on blur,
  so the ring is only ever on screen while the control is focused — a focus indicator in all but
  name. The plan's "known soft spot" did not materialise.
- **The two shadcn generations remain unmerged**, as the plan directed.

## A trap worth recording

Tailwind v4 scans source files for class-name candidates and **cannot tell a comment from code**.
Writing the old utility name in the WHY-comment above the fix made Tailwind emit a live
`outline-color: color-mix(… 50% …)` rule into the shipped bundle — dead, since nothing used the
class, but it looks exactly like the fix failing to land when you grep the bundle. The same blindness
bit the test: its first draft read the alpha out of its own comment and three mutations survived.
Comments are stripped before parsing now, and the old value is spelled out nowhere in `index.css`.

## Out of scope, deliberately

The trend's number overflowing a narrow card; a combo's left axis title reading a hidden series' name
after a legend click; the absence of a keyboard route to per-cell drill, which `ChartTable`,
`PivotTable` and object detail lack equally and which is one decision rather than three patches; and
the Metabase-side feature gaps (scatter's Bubble size and value-axis block, trend's Comparisons, the
Compact label mode, Unpin from zero, pie's Show total).

## Self-review

- **The diff should be tiny and the measurement should not be.** If this turns into a large diff,
  something has been misdiagnosed — stop and report rather than pressing on.
- **Task 1 is investigation and ends in a report**, because the numbers this rests on are second-hand
  and the change is product-wide. The audit that produced them was careful and #412 corroborated it —
  but corroboration by a third party who re-derived from the same tokens is not the same as measuring
  the rendered indicator, which nobody has done.
- **Known soft spot:** the plan assumes darkening light `--ring` is sufficient and safe. If `--ring`
  turns out to double as a non-focus border anywhere, that assumption breaks and the fix becomes a new
  token instead — Task 1 Step 3 exists to find that out early.
- **Do not "improve" the focus styles while in here.** The two shadcn generations are worth
  unifying, and that is a separate piece of work with its own review; conflating it with a contrast
  fix would make both harder to judge.
