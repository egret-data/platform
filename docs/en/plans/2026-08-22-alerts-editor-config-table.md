# Alerts Editor Fields onto `ConfigTable` (package E) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the 28 hand-written `Label` + control + helper-paragraph stacks inside `ScheduleEditor.tsx`'s four sections into `ConfigTable` rows, so each field's help text sits in the Details column where the conventions put it.

**Architecture:** One file changes. Each of the four sections keeps its `ConfigSection` card and gains one `ConfigTable` inside it, with `ConfigTableGroup` rows where a section has genuine sub-groups. No component is created and none is modified. The page's width cap, the section nav, `DestinationList`, and the dirtiness mechanism are all untouched.

**Tech Stack:** React 18, TypeScript, shadcn/Radix, Tailwind v4 OKLCH tokens, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-22-dataapi-alerts-design.md` §4 (English canonical; Vietnamese mirror alongside). Read §4.2 before starting — it lists three changes an earlier revision of the spec demanded that are **deliberately not** in this plan, with the evidence for each.

## Global Constraints

- **The 30 existing tests in `src/routes/explore/__tests__/ScheduleDetail.test.tsx` must pass unchanged.** They drive this editor by accessible name. (An earlier revision of this plan said 25; the file has 30, measured.) This is the binding constraint of the whole plan — see "The one way to break this" below.
- **Every field keeps its current `id`,** and its `ConfigTableRow` passes that id as `htmlFor`, so `getByLabelText` keeps resolving. `ConfigTableRow` renders a `<label htmlFor>` **only** when `htmlFor` is passed; omit it and the field loses its accessible name.
- **Owners is the deliberate exception.** It has no `htmlFor` today and must keep none: its control is a `TagCombobox` carrying its own `aria-label`, and the file's comment says why — *"A label pointing at an id that no longer exists is worse than none — it silently stops focusing anything."* Pass its label as a plain `ReactNode` with no `htmlFor`.
- **Do not modify** `ConfigTable.tsx`, `ConfigTableRow`, `ConfigTableGroup`, `ConfigSection.tsx`, `DestinationList.tsx`, or `ScheduleDetail.tsx`. `git diff --exit-code` on each must be empty at the end of every task.
- The eslint baseline is **0 errors / 177 warnings** (`npx eslint .` from `frontend/`), measured 2026-08-22. A new warning is a regression. Re-measure at the start rather than trusting this number.
- `npx tsc --noEmit` must pass before every commit.
- Run the full frontend suite (`npx vitest run` from `frontend/`) before every commit.
- Inline styles use `var(--token)`, never `hsl(var(--token))`.
- Every conditional that guards a field today must survive verbatim. Several fields render only for one schedule type or one picker mode.

## The one way to break this, stated once

`ScheduleDetail.test.tsx` finds fields by their label text. `ConfigTableRow` puts the label in the Setting cell and the control in the Value cell, and the two are only connected by `htmlFor` → `id`. **A row that renders the right label and the right control but forgets `htmlFor` looks correct on screen and fails `getByLabelText`.** If a test that did not change starts failing, this is the first thing to check.

The inverse is also worth knowing: a passing suite does not prove the conversion is right, because the tests do not assert where the help text went. That is what the browser check is for.

## Why the field bodies below say "copy from the file"

The code blocks in this plan give the **row structure** — the `ConfigTableRow`, its
`label`, its `htmlFor`, its `details` — and then say `{/* the existing control, unchanged */}`
where the control goes. That is deliberate, and it is a correction rather than laziness.

The preceding package's plan quoted control markup inline. Four of those quotations were
wrong: two had the wrong literal (`Create` for `+ Chart`, `renderAt` for `renderPage`), one
mutation check was a no-op, and one measurement counted a thing that had moved. Three were
caught by an instruction to read the file. **The fourth was not, and it shipped a behaviour
change** — the quoted snippet carried a `!showTrash` wrapper the real file did not have, the
implementer read it as the plan's structure with the file's literal, and a button silently
disappeared in the Trash view.

So: this plan states structure it has verified, and refuses to state structure it has not.
Where it says "the existing control, unchanged", open the file. Where it quotes a `details`
string as `…existing helper…`, the real sentence is in the file and this plan may have
transcribed it imperfectly.

## What this plan deliberately does NOT do

Read spec §4.2 for the evidence. In short: the `ConfigSection` card stays (its defect was *stacking*, and this page renders one card at a time); `ScheduleDetail.tsx`'s `mx-auto max-w-4xl` stays (it carries its own reason at `:114-116`, and all four sections here are the same single-column form); `DestinationList` stays (its rows are per-kind forms, not a listing).

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/components/alerts/ScheduleEditor.tsx` | **Modify.** All four sections. The only file this plan changes, apart from Task 5's documentation. |
| `frontend/src/routes/explore/__tests__/ScheduleDetail.test.tsx` | **Do not modify.** It is the regression gate. If a change here seems necessary, stop and report — it means the conversion changed behaviour. |
| `docs/en/ui-conventions.md` | **Modify, Task 5 only.** The `ConfigTable` consumer count changes. |

**Task order:** `general` first (3 fields, no conditionals) — its diff becomes the pattern the other three follow. Then `what` (7 fields, two schedule-type variants), then `notification` (7 fields), then `schedule` (11 fields and a conditional cron builder) last because it is the most intricate.

---

### Task 1: The `general` section — the reference conversion

**Files:**
- Modify: `frontend/src/components/alerts/ScheduleEditor.tsx`, the `{section === "general" && …}` block (currently around lines 412-489)

**Interfaces:**
- Consumes: `ConfigTable`, `ConfigTableRow` from `@/components/config/ConfigTable`.
- Produces: the row pattern the remaining three tasks copy. No exports change.

- [ ] **Step 1: Read what is there now**

Read the whole `{section === "general" && …}` block before changing anything. It holds three things: a two-up `grid grid-cols-1 sm:grid-cols-2` with `Name` and `Description`, an `Owners` block whose control is a `TagCombobox` with a helper paragraph, and an `Active` checkbox.

Read its comments too. Two of them explain decisions:

- the grid exists because *"a full-width row each left the section mostly white space with the fields stretched past any length they will ever hold"* — `ConfigTable`'s capped Value column solves that, so the grid goes;
- Owners has *no* `htmlFor` on purpose. Keep it that way.

- [ ] **Step 2: Run the tests first, to know what green looks like**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/ScheduleDetail.test.tsx`
Expected: all tests pass. **Write down the count.** This is the number that must not change.

- [ ] **Step 3: Add the import**

```tsx
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
```

- [ ] **Step 4: Convert the section body**

Replace the three blocks inside the `ConfigSection` with one `ConfigTable`. The `ConfigSection` element itself, its `level={2}`, its `title` and its `description` are unchanged.

```tsx
        <ConfigTable>
          <ConfigTableRow
            label="Name"
            htmlFor="sched-name"
            details="Shown in the list and used as the default email subject."
          >
            <Input id="sched-name" value={name} onChange={(e) => setName(e.target.value)} />
          </ConfigTableRow>

          <ConfigTableRow
            label="Description"
            htmlFor="sched-desc"
            details="Optional. A sentence for whoever finds this later."
          >
            <Input
              id="sched-desc"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </ConfigTableRow>

          {/* No `htmlFor`: the control is a combobox carrying its own
              `aria-label`, not an element with an id. A label pointing at an id
              that no longer exists is worse than none — it silently stops
              focusing anything. */}
          <ConfigTableRow
            label="Owners"
            details="The query runs with an owner's permissions. Leave empty and you become the owner; naming several keeps it running when one person leaves."
          >
            {/* the existing <TagCombobox …/>, every prop unchanged */}
          </ConfigTableRow>

          <ConfigTableRow
            label="Active"
            htmlFor="sched-active"
            details="Uncheck to pause without deleting. A paused schedule keeps its history."
          >
            <Checkbox
              id="sched-active"
              checked={active}
              onCheckedChange={(v) => setActive(v === true)}
            />
          </ConfigTableRow>
        </ConfigTable>
```

Two notes on the `details` strings above. The Owners one is the existing helper paragraph, moved verbatim — **copy it from the file rather than from this plan**, because this plan may have transcribed it imperfectly. The other three are new: those fields had no helper text, and the Details column is why the conversion is worth doing. Keep them to one sentence.

`Active` loses its `className="text-sm font-normal"` label styling, because `ConfigTableRow` styles the Setting cell itself. That is intended.

- [ ] **Step 5: Run the regression gate**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/ScheduleDetail.test.tsx`
Expected: the same count as Step 2, all passing. If `getByLabelText` fails for a field, the row is missing `htmlFor` or the control lost its `id`.

- [ ] **Step 6: Prove the labels are really connected**

A passing suite is not enough — a test could be finding text rather than a label. Delete `htmlFor="sched-name"` from the Name row, run `npx tsc --noEmit`, **confirm 0 errors**, then re-run the test file and confirm at least one test now fails. Restore it. A mutation that does not compile proves nothing, which is why the build check comes first.

- [ ] **Step 7: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/config/ frontend/src/components/form/ frontend/src/components/alerts/DestinationList.tsx frontend/src/routes/explore/ScheduleDetail.tsx
```

The `git diff --exit-code` must print nothing. If it prints a diff, a file this plan declared untouched has changed.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/alerts/ScheduleEditor.tsx
git commit -m "refactor(alerts): the General section's fields onto ConfigTable

Help text moves from a paragraph wedged under each control into the Details
column. The two-up Name/Description grid goes: it existed because a full-width
row let the inputs stretch past any length they hold, and ConfigTable's capped
Value column solves that directly."
```

---

### Task 2: The `what` section — two variants, one wide control

**Files:**
- Modify: `frontend/src/components/alerts/ScheduleEditor.tsx`, the `{section === "what" && (isAlert ? … : …)}` block (currently around lines 491-677)

**Interfaces:**
- Consumes: `ConfigTable`, `ConfigTableRow` — already imported by Task 1.
- Produces: nothing new.

This section renders **two different `ConfigSection`s**, chosen by `isAlert`: "Alert condition" (Database, SQL, Condition) and "Contents" (Deliver, target, an "Also include" list, Format). Both convert; they are separate tables inside separate cards.

- [ ] **Step 1: Read the block and inventory every conditional**

Read the whole `{section === "what" && …}` block. Before writing anything, write down every condition that decides whether a field renders — the `targetKind === "dashboard"` label switch, the "Also include" extras loop with its `sched-extra-${i}` ids, the validator's threshold `Input` that only appears for one validator type, and the error paragraph under SQL. Each must survive with the same condition.

- [ ] **Step 2: Widen the Value column for this table**

The SQL field is a `Textarea`. In a 208px Value column it is unusable. `ConfigTable` takes `valueClassName`, and it must be set on the table rather than on the cell's contents:

```tsx
<ConfigTable valueClassName="w-96">
```

`ConfigTable`'s own doc comment states why this cannot go in the caller's cell: *"under `table-layout: fixed` a column is exactly as wide as its `<th>` says, so a `w-96` wrapper inside a `w-52` cell overflows into its neighbour instead of widening the column."*

Apply `valueClassName="w-96"` to the alert table only. The report table has no wide control; leave it at the default.

- [ ] **Step 3: Convert the alert variant**

Three rows, each carrying the field's existing `id` as `htmlFor`:

```tsx
        <ConfigTable valueClassName="w-96">
          <ConfigTableRow label="Database" htmlFor="sched-db" details="Which connection the query runs against.">
            {/* the existing database control, unchanged */}
          </ConfigTableRow>

          <ConfigTableRow
            label="SQL"
            htmlFor="sched-sql"
            details="…the existing helper paragraph, copied from the file…"
          >
            {/* the existing <Textarea id="sched-sql" …/>, unchanged, and the
                error paragraph that sits under it — it belongs with the control
                in the Value cell, not in Details, because it is a validation
                result rather than help. */}
          </ConfigTableRow>

          <ConfigTableRow label="Condition" htmlFor="sched-validator" details="What about the result makes this fire.">
            {/* the existing validator control AND its conditional threshold
                Input, both in this one cell, with the condition unchanged */}
          </ConfigTableRow>
        </ConfigTable>
```

The SQL error paragraph (`className="text-xs text-destructive"`) stays in the **Value** cell beneath the control. Details is for help that is always true; an error is about this attempt.

- [ ] **Step 4: Convert the report variant**

Four rows in a plain `<ConfigTable>` (no `valueClassName`): `Deliver` (`sched-target-kind`), the target picker (`sched-target`, whose label is a `targetKind === "dashboard" ? … : …` expression — keep the expression, `ConfigTableRow`'s `label` takes a `ReactNode`), the "Also include" extras, and `Format` (`sched-format`, which has a helper paragraph — move it to `details`).

The extras are a loop over `sched-extra-${i}`. Render **one `ConfigTableRow` per extra**, each with `htmlFor={`sched-extra-${i}`}`, so each keeps its own label association. Do not put the whole loop in one row.

- [ ] **Step 5: Run the regression gate**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/ScheduleDetail.test.tsx`
Expected: the same count as Task 1 Step 2, all passing. Several tests in this file exercise the report variant specifically — "sends a report body with a target rather than alert fields", "forces PDF once a report covers more than one object", "omits an absent database rather than sending an empty string".

- [ ] **Step 6: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/config/ frontend/src/components/form/ frontend/src/components/alerts/DestinationList.tsx frontend/src/routes/explore/ScheduleDetail.tsx
```

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/alerts/ScheduleEditor.tsx
git commit -m "refactor(alerts): the Alert condition and Contents sections onto ConfigTable

The alert table sets valueClassName=w-96 because the SQL Textarea is unusable in
the default 208px Value column, and under table-fixed a wrapper inside the cell
would overflow its neighbour rather than widen the column."
```

---

### Task 3: The `notification` section

**Files:**
- Modify: `frontend/src/components/alerts/ScheduleEditor.tsx`, the `{section === "notification" && …}` block (currently around lines 918-1104)

**Interfaces:**
- Consumes: `ConfigTable`, `ConfigTableRow`, `ConfigTableGroup` from `@/components/config/ConfigTable`.
- Produces: nothing new.

Seven fields plus `DestinationList`. The fields split into two natural groups, which is what `ConfigTableGroup` is for.

- [ ] **Step 1: Read the block, and note what is not a field**

`<DestinationList value={destinations} onChange={setDestinations} />` (currently around line 928) is **not** a `ConfigTableRow`. It is a composed list of per-kind forms — spec §4.2 says why it stays as it is. Render it **outside and above** the `ConfigTable`, still inside the `ConfigSection`. `ConfigSection` puts `space-y-4` between its children, so it will space correctly with no wrapper.

Also note the "Email subject" field has a row of token-insert buttons beside it (`title={`Insert ${t.hint}`}`). Those belong in the **Value** cell with the input, not in Details.

- [ ] **Step 2: Convert, with two groups**

```tsx
        <DestinationList value={destinations} onChange={setDestinations} />

        <ConfigTable valueClassName="w-96">
          <ConfigTableGroup label="Message" />

          <ConfigTableRow label="Email subject" htmlFor="sched-subject" details="…existing helper, copied from the file…">
            {/* the existing input AND its token-insert buttons */}
          </ConfigTableRow>

          <ConfigTableRow label="Opening note" htmlFor="sched-message" details="…existing helper…">
            {/* existing Textarea */}
          </ConfigTableRow>

          <ConfigTableRow label="Closing note" htmlFor="sched-message-footer" details="…existing helper…">
            {/* existing Textarea */}
          </ConfigTableRow>

          <ConfigTableGroup label="Attachment" />

          <ConfigTableRow label="Also show a chart or dashboard" htmlFor="sched-attach" details="…">
            {/* existing checkbox */}
          </ConfigTableRow>

          <ConfigTableRow label="Image width (px)" htmlFor="sched-width" details="…">
            {/* existing input, with its rendering condition unchanged */}
          </ConfigTableRow>

          <ConfigTableRow label="Image height (px)" htmlFor="sched-height" details="…existing helper…">
            {/* existing input, with its rendering condition unchanged */}
          </ConfigTableRow>

          <ConfigTableGroup label="Delivery" />

          <ConfigTableRow label="Don't send when there are no rows" htmlFor="sched-skip-empty" details="…">
            {/* existing checkbox */}
          </ConfigTableRow>
        </ConfigTable>
```

The width and height rows are conditional today — they render only when the attachment toggle is on. **Keep that condition**, wrapping the two `ConfigTableRow`s exactly as the two blocks are wrapped now. A `ConfigTableGroup` whose rows are all hidden leaves a stray heading; if the `Attachment` group can be empty, put the group row inside the same condition as the rows it heads.

`ConfigTableGroup` spans all three columns and carries `role="rowheader"`. Do not give it a `label` that duplicates the `ConfigSection` title.

`valueClassName="w-96"` because two of these are `Textarea`s, for the same reason as Task 2.

- [ ] **Step 3: Run the regression gate**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/ScheduleDetail.test.tsx`
Expected: the same count, all passing. Five tests here exercise destinations — "keeps a webhook destination through a no-op save", "sends a webhook as a bare url, never a credential", "sends a slack destination as a channel, never a token in the row", "delivers to several destinations of the same kind". If any of those fail, `DestinationList` was moved or wrapped in a way that changed it.

- [ ] **Step 4: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/config/ frontend/src/components/form/ frontend/src/components/alerts/DestinationList.tsx frontend/src/routes/explore/ScheduleDetail.tsx
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/alerts/ScheduleEditor.tsx
git commit -m "refactor(alerts): the Notification section onto ConfigTable

Three ConfigTableGroup rows — Message, Attachment, Delivery — inside one table.
DestinationList stays as it is and sits above the table: its rows are per-kind
forms, not a listing, so spec section 4.2's rule does not describe it."
```

---

### Task 4: The `schedule` section — the conditional cron builder

**Files:**
- Modify: `frontend/src/components/alerts/ScheduleEditor.tsx`, the `{section === "schedule" && …}` block (currently around lines 678-916)

**Interfaces:**
- Consumes: `ConfigTable`, `ConfigTableRow`, `ConfigTableGroup`.
- Produces: nothing new.

Eleven fields, and the intricate one. `Cron expression` and the picker (`Repeat` / `Every` / `On` / `On day` / `At`) are two alternative ways of saying the same thing, and which set renders depends on the editor's mode. The three timers at the bottom are independent.

- [ ] **Step 1: Read the block and write down the mode logic**

Before converting anything, write down — in your report — exactly which condition decides whether the raw `sched-cron` input renders versus the picker, and which of `Every` / `On` / `On day` / `At` render for each value of the period. There are **two** fields with `htmlFor="sched-every"` in the current code, in mutually exclusive branches. Note which branch each belongs to; both must keep that id and both must keep their branch.

The test `"keeps the raw cron field for an expression the picker cannot express"` is the one that pins this behaviour. Read it before you start.

- [ ] **Step 2: Convert, with two groups**

```tsx
        <ConfigTable>
          <ConfigTableGroup label="When it runs" />

          {/* every field of the cron half — the raw `sched-cron` input and the
              picker's controls — each as its own ConfigTableRow carrying its
              existing id as htmlFor, and each wrapped in the SAME condition it
              carries today. Copy the conditions from the file; do not
              reconstruct them. */}

          <ConfigTableRow label="Timezone" htmlFor="sched-tz" details="Runs are scheduled in this zone, and dates in the message are rendered in it.">
            {/* existing control */}
          </ConfigTableRow>

          <ConfigTableGroup label="Limits" />

          <ConfigTableRow label="Grace period (seconds)" htmlFor="sched-grace" details="How long to wait before firing again for the same condition.">
            {/* existing input */}
          </ConfigTableRow>

          <ConfigTableRow label="Working timeout (seconds)" htmlFor="sched-timeout" details="A run still working after this is marked failed.">
            {/* existing input */}
          </ConfigTableRow>

          <ConfigTableRow label="Run log retention (days)" htmlFor="sched-retention" details="…existing helper, copied from the file…">
            {/* existing input */}
          </ConfigTableRow>
        </ConfigTable>
```

The `Cron expression` field has a helper paragraph today — move it to `details`. So does `Run log retention (days)`.

If the picker's controls read better as one row than as five (they are a sentence — "every 2 weeks on Tuesday at 09:00"), putting the whole picker in a single `ConfigTableRow` labelled `Repeat` is acceptable **provided every control keeps its id and the row keeps `htmlFor="sched-period"`**. State which you chose and why in your report. Do not split a control away from the condition that renders it.

- [ ] **Step 3: Run the regression gate**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/ScheduleDetail.test.tsx`
Expected: the same count, all passing. Two tests target this section directly — `"sends the timers and retention the editor previously had no way to set"` and `"keeps the raw cron field for an expression the picker cannot express"`.

- [ ] **Step 4: Prove the mode switch still works**

The suite covers the raw-cron branch. Confirm the picker branch by hand: change the period in the running app and check that the controls which appear match what the old code showed for that period. If the app is not running, say so in your report rather than claiming it was checked.

- [ ] **Step 5: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/config/ frontend/src/components/form/ frontend/src/components/alerts/DestinationList.tsx frontend/src/routes/explore/ScheduleDetail.tsx
```

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/alerts/ScheduleEditor.tsx
git commit -m "refactor(alerts): the Schedule section onto ConfigTable

Two ConfigTableGroup rows, When it runs and Limits. Every condition that decides
whether a cron control renders is carried over unchanged, including the two
mutually exclusive fields that share the id sched-every."
```

---

### Task 5: Re-measure the `ConfigTable` consumer count

**Files:**
- Modify: `docs/en/ui-conventions.md`, the `## Adoption status` section

**Interfaces:**
- Consumes: the converted file from Tasks 1-4.
- Produces: nothing consumed by code.

- [ ] **Step 1: Re-measure**

```bash
cd frontend
grep -rl 'from "@/components/config/ConfigTable"' src --include='*.tsx' | grep -v __tests__ | wc -l
grep -rl 'from "@/components/form/ConfigSection"' src --include='*.tsx' | grep -v __tests__ | wc -l
```

Quote the `--include` glob. Unquoted, it fails under zsh with `no matches found`, and a command in this document that cannot be run is worse than no command.

- [ ] **Step 2: Update the two bullets with the measured numbers**

The section records `ConfigTable` at 15 and `ConfigSection` at 13 as of the previous phase. **`ConfigSection` should be unchanged** — this package keeps the card. If your measurement disagrees, do not edit it to match the expectation: report the discrepancy, because it means something else changed.

Add one sentence naming `ScheduleEditor.tsx` as a new `ConfigTable` consumer, and note that it is the first consumer that keeps its `ConfigSection` frame — the frame is not a defect there, because the page renders one section at a time.

- [ ] **Step 3: Re-check the counts this package did not touch**

`EmptyState`, `ListPageShell`, `ListStates`, the plain-`Table` list of §4.3, and the §11 split are not affected by this work. Re-run each command printed beside them and confirm the numbers are unchanged. If one has drifted, report it; do not silently edit it.

- [ ] **Step 4: Commit**

```bash
git add docs/en/ui-conventions.md
git commit -m "docs(ui): record ScheduleEditor as a ConfigTable consumer

It is the first that keeps its ConfigSection frame, because the page renders one
section at a time and the frame's defect was stacking."
```

---

## Browser check, once, at the end

The test suite cannot see where the help text went, which is the entire point of this package. After Task 5, open the editor and look at all four sections:

- Each field's help text is in the Details column at `lg` and above, and falls back to sitting under the Setting label below `lg` (that fallback is `ConfigTableRow`'s own behaviour — confirm it happens rather than assuming).
- The SQL `Textarea` and the two note `Textarea`s are usable at `w-96`, not squeezed.
- The `Attachment` group heading does not appear alone when the attachment toggle is off.
- Switching sections still keeps typed values — the suite covers this, but confirm it live too, because it is the behaviour the section nav exists to preserve.
- The page still measures 4xl wide, and the header, tab bar and form still share one left edge.

## Self-review notes

Checked against spec §4 before saving:

- §4.1's positive rule (fields become `Setting | Details | Value` rows; help text moves to Details) — Tasks 1-4, one per section.
- §4.1's note that the `Name`/`Description` grid converts because `ConfigTable` caps the Value column — Task 1 Step 4, with the reason inline.
- §4.2's three keeps (`ConfigSection`, `max-w-4xl`, `DestinationList`) — stated in Global Constraints, enforced by the `git diff --exit-code` in every task's gate, and `DestinationList`'s placement is spelled out in Task 3 Step 1.
- §4.3's two do-not-touch items (dirtiness by comparison, one column with no self-width) — neither is inside any section body, so no task reaches them; the `git diff` gate does not cover them, which is why they are named in Global Constraints.
- The `htmlFor` constraint has no counterpart in the spec because it is an implementation consequence, not a design decision. It is stated once, in its own section, because it is the single way this plan can break 30 tests that nobody edited.
