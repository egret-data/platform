# Put `LabelLayout` in the bundle — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `labelLayout: { hideOverlap: true }` do something. Six files set it, and none of them
gets it in the shipped app — including a setting the panel offers a user by name.

**Architecture:** One line in `echarts/register.ts`. The work is not the change; it is establishing
what the change does to five chart families before shipping it.

**Tech stack:** TypeScript, ECharts 6.1, vitest.

---

## The defect

ECharts installs label layout as a top-level `use(installLabelLayout)` in `lib/export/core.js`. That
path is **not** in echarts' `package.json` `sideEffects` allowlist (`index*.js`, `lib/echarts.js`,
`lib/chart/*`, `lib/component/*`, `extension/**`, `theme/*`, `i18n/*`), so Rollup drops the call.

Confirmed in `frontend/dist/assets/index-*.js`: `shiftLayoutOnXY`, `labelGuideIgnore`, `hostRect` and
`installLabelLayout` all return **zero** hits — `LabelManager.js` is not in the bundle at all.

**vitest and `vite dev` do not tree-shake**, so every unit test and every dev session shows the
backstop working. Two SSR tests and a standalone Chromium page confirmed `hideOverlap` itself works
fine for a `sunburst` — the mechanism is sound, the build drops it.

**`register.ts` already carries the precedent, in its own words:**

> The aria component is not re-exported from "echarts/components", and this deep entry self-registers
> on import. Without it every `aria` option — the screen-reader description AND the decal patterns —
> is silently ignored, which is exactly what happened here until the accessibility audit went looking
> for the label and found none.

Same class of bug, same file, found the same way.

## What it actually reaches — the risk is asymmetric

| file | what `hideOverlap` governs | on by default? |
|---|---|---|
| `options/cartesian.ts:991` | series **value labels**, when `labelDensity !== "all"` | **no** — `showLabels` is `!!config.showLabels` |
| `options/waterfall.ts:320` | value labels, `labelDensity ?? "fit"` | **no** |
| `options/boxplot.ts:427` | mean-point value labels | **no** |
| `options/pie.ts:1710` | slice labels | **yes** — `cfg.showLabels ?? true` |
| `options/sankey.ts:354` | node labels | **yes**, always drawn |

So:

- **The value-label half is a setting that lies.** `CartesianDisplaySection.tsx:427`,
  `WaterfallDisplaySection.tsx:76` and `BoxplotDisplaySection.tsx:75` offer a `fit` / `all` density
  select the moment a user turns values on. `fit` and `all` currently render **identically**. That is
  the user-visible half, and enabling the component is invisible until someone opts in — low risk.
- **The pie and sankey half changes what is already on screen**, with no user action. That is where
  the risk is, and Task 1 exists to measure it.

## What must NOT change

PRs 05 and 05b made the pie's and the sankey's label decisions **geometric and primary**, precisely
because `hideOverlap` was dead:

- `sliceLabel.ts` measures each label against its own slice's chord and truncates or drops it —
  `calcAvailableFullPieSliceLabelLength`, ported from Metabase.
- `sankey.ts` reserves a right gutter for the widest last-column label and truncates at a 25% cap,
  ported from Metabase's `getSankeyLayout`.

Those stay primary. `hideOverlap` is a backstop for what survives them and still collides.
`options/pie.ts:1706-1709` already says so:

> Every decision about what fits is made above, where it can be measured; this line is what the app
> gets back for free if that install is ever restored, and costs nothing meanwhile.

**If enabling it drops a pie or sankey label that the geometric fit deliberately kept, that is a
regression of a measured decision** — report it rather than accepting it, because those decisions were
taken against live measurements and Metabase's own behaviour.

---

## Task 1: establish what changes — MEASURE BEFORE CHANGING

**No permanent change in this task. Report, then we decide.**

- [ ] **Step 1: Confirm the defect is still present.** Build, then check the shipped bundle for
  `shiftLayoutOnXY`, `labelGuideIgnore`, `hostRect`, `installLabelLayout`. If any are now present, stop
  — something else changed and this plan needs rewriting.

- [ ] **Step 2: Find the right import.** `echarts/features` exports `LabelLayout` (echarts 6.1.0 is
  what is installed). Confirm the exact export and that adding it to `register.ts`'s `use([...])` is
  sufficient — read what `installLabelLayout` registers.

- [ ] **Step 3: Build both ways and diff the rendered output.** With and without the registration,
  for **each of the five families**, at several sizes — at minimum the harness's own 166×184 card and
  something roomy (640×400), because a backstop that never fires at one size may fire at another.

  Render through real ECharts and read the labels back out of the SVG; the `.echarts.test.ts` files
  are the precedent. Report, per family and size, **which labels disappear**.

  Cover the value-label families with `showLabels: true` and both `labelDensity: "fit"` and `"all"` —
  the whole point is that those two currently render the same and should not.

- [ ] **Step 4: Report the bundle-size delta.** `LabelManager` is not free; say what it costs.

- [ ] **Step 5: Report and stop.** State plainly: what improves, what disappears that used to be
  drawn, and whether any of the disappearances contradict a decision PRs 05/05b took deliberately.

---

## Task 2: enable it

Only after Task 1's report.

- [ ] **Step 1: Write the failing test.** The honest one is hard and worth thinking about: a unit test
  cannot see the bundle. Options, in the order I would consider them:
  - a test that renders through **real ECharts in SSR** and asserts that an overlapping label is
    dropped — this passes today, because vitest does not tree-shake, so it is **not** a regression
    test for the defect;
  - a **build-output assertion**: after `npm run build`, the bundle contains the LabelManager symbols.
    This is the only check that can actually fail for the reason we care about. Decide whether it
    belongs in the test suite or in CI, and say why.

  **Do not write a test that passes for the wrong reason.** If the only honest unit-level assertion is
  weak, say so and let Task 3's live check carry the criterion. This programme has shipped a passing
  suite over a dead code path twice.

- [ ] **Step 2: Add `LabelLayout` to `register.ts`**, with a comment saying why it is listed explicitly
  — the `sideEffects` omission — so nobody removes it as redundant.

- [ ] **Step 3:** `cd frontend && npx tsc --noEmit && npm run lint && npm run lint:harness && npx vitest run`.

  **Existing tests may now fail**, because five families start hiding labels. Every one that moves must
  be **understood**: is the new expectation right, or did the backstop just undo a geometric decision?
  Say which for each.

- [ ] **Step 4: Commit**

```bash
git add frontend
git commit -m "fix(charts): the label-overlap backstop reaches the bundle"
```

---

## Task 3: the live comparison, and the PR

**This is the acceptance test.** The defect exists precisely because tests and dev sessions disagree
with the shipped app, so a green suite proves less here than usual.

- [ ] **Step 1: Gates**, then `npm run build`, then **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js`. A stale `index.html` has produced a wrong conclusion twice
  in this programme.

  A static server on :5174 serves `frontend/dist` and proxies `/api` to `127.0.0.1:30080` (`serve.py`
  in the session scratchpad). Log in at `/login` → "Admin sign-in (break-glass)" →
  `admin@egret.local` / `Admin12345!`, rate-limited 5 per 5 minutes.

- [ ] **Step 2: Confirm the bundle now contains it.** The same four symbols from Task 1 Step 1.

- [ ] **Step 3: Prove the setting works.** Open `MB · Bar`, turn on "Data labels", and compare density
  `fit` against `all` at a narrow width. **They must now differ** — today they do not. Report both
  label sets.

- [ ] **Step 4: Nothing on the dashboard regressed.** Measure all 19 charts and compare against the
  tables PRs 02–07 recorded: the sankey's `Doo… Wid… Gad… Gizmo` and nine colours, the pie drawing no
  slice labels at 166×129, the row chart's `0 / 500K`, the gauge's single graduation, the pivot's
  14 rows, the temporal axes reading `January 2026`, and bar/row/box plot category order.

  **A label that disappears here is a finding, not automatically a win.** If the pie or sankey loses a
  label the geometric fit kept, report it with the size it happens at.

- [ ] **Step 5: Open the PR.** Body carries Task 1's before/after table, the bundle-size delta, the
  `fit` vs `all` evidence, and any label that disappeared with a judgement on whether it should have.
  Push and open. **Do not merge.** Stop and report.

---

## Self-review

- **The change is one line; the plan is nearly all measurement.** That is deliberate — the risk is not
  that the line is wrong, it is that five chart families change at once and two of them change without
  any user action.
- **Task 1 is investigation-only and ends in a report**, for the same reason PR 05's pie task and
  PR 05b's pivot task did: two mechanisms already exist here (the geometric fits), and enabling a third
  without knowing what it overrides is how a measured decision gets silently undone.
- **The testing problem is named rather than papered over.** A unit test cannot fail for this defect,
  because the defect only exists in a tree-shaken build. Whatever Task 2 writes, the PR must say what
  it does and does not prove.
- **Known soft spot.** Task 1 asks for a rendered diff across five families and two sizes, which is
  ten comparisons. If that turns out to need more sizes to be meaningful — a backstop is size-dependent
  by nature — say so and widen it rather than reporting a thin sweep as complete.
