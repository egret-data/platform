# Bring the Rest of the Endpoint Detail Sections Into the Same Style

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** The four remaining endpoint sections read like the two that have been redone — tables instead of stacked cards, and a resizable split wherever the shape is "set something up, run it, read the result".

**Architecture:** Apply the idioms already proven in `Config` (#521) and `Query` (#529) rather than inventing anything new.

**Tech Stack:** React 18, TypeScript, `react-resizable-panels` v4, Tailwind v4.

## Measured starting state

| section | lines | `<Card>` | tables | note |
|---|---|---|---|---|
| `Config.tsx` | 367 | **0** | 17 | done in #521 — the reference |
| `Query.tsx` | — | 0 | — | done in #529 — the other reference |
| `ResponseSection.tsx` | 227 | **8** | 4 | half done; two cards were kept **deliberately** |
| `AccessSection.tsx` | 495 | **4** | 0 | not started |
| `TryIt.tsx` | 339 | **8** | 0 | not started; most cards of any section |
| `Versions.tsx` | 330 | **4** | 0 | not started |

`TryIt` is `Run as` → `Result`, which is the same "configure, run, read" shape `Query` has. `Versions` is a history list. `Versions`'s single `<Textarea rows={2}>` is a **publish note**, not SQL — it does not want a code editor.

## Global Constraints

- **`docs/en/ui-conventions.md` is the canonical rulebook** this programme produced. Read it before adding anything to a section, and update it if you establish a new rule.
- **`ConfigTable` (`Setting | Details | Value`) is for settings-shaped content; a plain `Table` is for list-shaped content** (service accounts, versions, grants). Do not force a list into `ConfigTable`.
- **This is a re-presentation, not a feature cut.** Every control, empty state, error state and piece of explanatory copy that exists today must survive. If something genuinely has no place in a table, say where you put it and why.
- **Do not reverse a deliberate decision without reading it first.** `ResponseSection` kept two cards on purpose in #521. Find the reasoning, then either follow it or state plainly why it no longer holds. "Consistency" is not by itself an argument that beats a recorded rationale.
- eslint baseline **0 errors / 177 warnings**; `tsc --noEmit` clean. No msw.
- Run the frontend suite unpiped. NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file.
- Tokens are OKLCH (Tailwind v4): inline styles use `var(--token)`, never `hsl(var(--token))`.
- **Commit before cutting any mutation.** Typecheck to zero errors, CONFIRM the patch landed, then read the result.
- **A green vitest run is not a typecheck.** A fixture with a wrong field name passed its own test in #529 and only `tsc` caught it. Run both.

## Traps already paid for — do not rediscover these

- **The shell wraps every section in `<fieldset disabled={trashed}>`, which disables every button under it.** A Radix collapsible's trigger is a `<button>`, so a trashed endpoint could not collapse it. Native `<details>`/`<summary>` is exempt and is what `Query.tsx` uses.
- **`react-resizable-panels` v4**: sizes are **strings** (a bare number means pixels), and the prop is **`orientation`**, not `direction`.
- **In jsdom the panel `Separator` wins the hit-test with 0×0 rects and steals focus**, which broke 14 unrelated typing tests in #529. `Query.test.tsx` already has the stub — reuse its approach.
- **jsdom does no layout and does not hide a closed `<details>`.** Assert the `open` **attribute**, never visibility, and do not write tests that pretend to verify geometry. The browser pass is the real check.

---

### Task 1: Try it

**Files:** `frontend/src/routes/explore/dataapi/detail/TryIt.tsx`, its tests

`Run as` and `Result` are the same shape `Query` has, so it gets the same treatment: the setup in tables, the result in a panel below a draggable divider, and a keyboard shortcut to run.

- [ ] **Step 1: Settings into tables**

The service-account picker and anything else that configures the call become `ConfigTable` rows. Parameter **values** for the trial are list-shaped — a plain table, one row per parameter.

- [ ] **Step 2: The split**

`Query.tsx` is the working example, including its `<details>` strip. Reuse the shape rather than re-deriving it. Wire a run shortcut and **show it in the result panel's header**, as `Query` does — an affordance nobody can see is not an affordance.

- [ ] **Step 3: Keep the distinction Try-it exists to make**

Try it runs as the **service account**; the preview in `Query` runs as **the author**. `PreviewBlock`'s note says so explicitly, and the two were proven to differ live on kind (`who=admin` vs `who=svc-kind-smoke-bot`). Whatever copy carries that distinction today must survive the reshape — it is the entire reason both features exist.

- [ ] **Step 4: Test, mutation-check, commit**

Assert the **payload** the trial request carries, not merely that it fired. Use a fixture with **two** parameters so editing the wrong row is distinguishable from editing the right one.

---

### Task 2: Versions and Access

**Files:** `frontend/src/routes/explore/dataapi/detail/Versions.tsx`, `AccessSection.tsx`, their tests

Both are list-shaped, so both get plain tables, not `ConfigTable`.

- [ ] **Step 1: Versions**

`History` becomes a table — one row per frozen snapshot. Keep which version is live obvious; it is the one fact a reader scans this list for. Publish and roll-back must stay reachable per row, and the trashed-endpoint guard on both must survive (#522 added it, and `Versions.tsx:209`/`:269` are the controls).

The publish note stays a `<Textarea rows={2}>` — it is prose, not code.

- [ ] **Step 2: Access**

`Service accounts` and its sibling cards become tables. Grants are list-shaped: one row per grant, with the revoke control on its row.

- [ ] **Step 3: Test, mutation-check, commit**

For each: assert the id passed to the mutation, with at least two rows in the fixture.

---

### Task 3: Finish Response, or say why not

**Files:** `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx`, its tests

Eight cards remain, and **two of them were kept deliberately in #521** (`Frozen columns` and the OpenAPI document). 

- [ ] **Step 1: Read the recorded reasoning first**

Find why those two stayed. Then decide, and **write the decision down either way**: convert them, or leave them with the rationale restated in the code so the next reader does not have to re-litigate it.

- [ ] **Step 2: Convert the rest**

- [ ] **Step 3: Test, mutation-check, commit**

---

## After the plan: check it in a browser

Layout is the class of change jsdom is least able to judge, and the browser found a real defect in #529 (an editor squeezed to 107px) that every test had passed.

For each of the four sections: it fills the height without double-scrolling, the tables are readable at a normal window width, and nothing that used to be reachable has become unreachable. For Try it specifically: the split drags, the shortcut runs the trial, and the result lands in the bottom panel.

## Self-Review

**Spec coverage:** the four sections map to Tasks 1-3. **Placeholder scan:** Task 3 deliberately does not prescribe an outcome, because the honest answer depends on a rationale I have not read; it prescribes reading it and recording a decision. **Type consistency:** `ConfigTable` for settings, `Table` for lists, throughout.
