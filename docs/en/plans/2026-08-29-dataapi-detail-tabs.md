# Data API endpoint detail — the five tabs that are not Config

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to
> implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the six endpoint-detail tabs read as one page. Today only **Config** does.

**Architecture:** No new design language. `Config.tsx` already proves the shape — **one table, with
grouping inside it** — and the other five stack differently-shaped blocks instead. This plan applies
Config's shape outward and settles the width question that makes the tab strip jump.

**Tech Stack:** React 18 + TypeScript, shadcn/Radix, Tailwind v4 with OKLCH tokens, Monaco,
`react-resizable-panels` v4, vitest + Playwright.

## What was measured, on the deployed build at 1600×1000 with seeded data

| Tab | Content available | Widest block | Distinct block widths |
|---|---|---|---|
| Query | 1296 | 1296 | 1296, 419 |
| **Config** | 1296 | 1296 | **1296 only** |
| Response | 1296 | 1296 | 1296, 532 |
| Access | 1296 | **1024** | 1024 |
| Try it | 1296 | **1024** | 1016, 893 |
| Versions | 1296 | **1024** | 1024, 823 |

Three findings follow from that table and from the screens:

1. **The content width jumps 272px between tabs** — three use the page, three cap at 1024.
2. **Config is the only tab with a single content measure.**
3. **Config groups *inside* one table** (`ENDPOINT` / `CACHE` / `LIMITS` / `RATE LIMIT` as group
   rows). The others group by stacking blocks whose flat `text-sm` labels carry no hierarchy.

Two concrete defects only density exposed:

4. **Access rows are ~79px** because `Try it` is stacked **below** `Grant invoke` rather than beside
   it; seven accounts fill the viewport, and every email truncates at `w-48`.
5. **Try it clips its own parameter table**: the pane holding it reports `clientHeight 237` against
   `scrollHeight 297` — 60px hidden behind an inner scroll — while roughly 400px of viewport sits
   empty below the section.

## Global Constraints

- **`docs/en/ui-conventions.md` is canonical, and this plan REVERSES one of its rulings.** §5.1
  currently says Access, Try it and Versions cap themselves because they "read worse stretched".
  The user has ruled for **one width across all six**. Record the reversal and the reason — the
  layout jump between sibling tabs outweighs the per-section reading argument — do **not** silently
  edit the old text away. §5.6's count and `__tests__/sectionWidth.ts`'s two helpers move with it.
- **Explore idiom**, not control-plane: `p-4`, one `h1`, `text-lg`.
- **OKLCH tokens only**; `--accent` is reserved for hover/focus; no raw Tailwind palette colours.
- Gates from `frontend/`: `npx vitest run && npx tsc --noEmit && npx eslint .` → green, tsc 0,
  **eslint 177 warnings / 0 errors**. Report exact numbers. **No prettier.**
- **jsdom does no layout.** Every width, height, clipping or alignment claim needs a **real
  browser**; a jsdom test for one passes either way. `tests/e2e/data-api-response-width.spec.ts`
  and `data-api-narrow-and-cells.spec.ts` are the precedents, and both are opt-in behind an env
  var because CI runs no Playwright step.
- **A `<details>` is exempt from `fieldset[disabled]`; a Radix collapsible is not.** The detail page
  disables its fieldset for a trashed endpoint, and collapsing is reading, not editing.
- **`ConfigTable`'s Value cell needs `whitespace-normal break-words`** — it inherits
  `whitespace-nowrap` from `table.tsx`, and prose placed there overflows silently.
- **Test traps this repo has lost tests to:** a click is not an observable; `toBeUndefined()` passes
  on a never-set field; `toBeDisabled()` walks up to an ancestor `fieldset[disabled]`; an
  accessible-name assertion must use the real computation; a test can pin a **library** rather than
  your file; and a **degenerate fixture** cannot tell right from wrong.
- **Commit before mutating**, confirm every mutant **and the baseline** compiles, and **re-verify
  the tree after every revert** — reverts have silently failed four times in this programme, and
  `cp -i` has refused one outright.

---

### Task 1: One width, and a documented reversal

**Files:**
- Modify: `detail/AccessSection.tsx:245`, `detail/TryIt.tsx:241`, `detail/Versions.tsx:89`
- Modify: `__tests__/sectionWidth.ts`, `__tests__/AccessSection.test.tsx:775`,
  `__tests__/TryIt.test.tsx:642`, `__tests__/EndpointDetail.test.tsx:1852,1873,1934`
- Modify: `docs/en/ui-conventions.md` §5.1, §5.6

Remove `max-w-5xl` from the three sections so all six leave width to the page.

`sectionWidth.ts` exports `expectSectionCapsItsOwnWidth` and `expectSectionLeavesWidthToThePage`.
After this task **nothing caps**, so decide deliberately: delete the capping helper, or keep it
unused for a future section. Do not leave a helper whose only callers were deleted without saying
which you chose and why.

**The doc edit is the load-bearing part.** §5.1 argued the opposite of what ships after this task.
Rewrite it to state the new rule, keep the old argument visible as a superseded ruling with the
reason it lost, and fix §5.6's count. A reader must not find a section describing a rule the
reference implementation violates — that has happened twice in this file already.

- [ ] **Step 1: Write the failing tests** — flip the three assertions first and watch them fail
- [ ] **Step 2: Run them and confirm each fails for its own reason**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove it in a real browser** — measure all six tabs' widest block at 1600×1000 and
      assert they agree; a jsdom class assertion is not evidence here
- [ ] **Step 5: Commit**

---

### Task 2: Access — a row that fits, and an identity you can read

**Files:**
- Modify: `detail/AccessSection.tsx`
- Test: `__tests__/AccessSection.test.tsx` + a real-browser check

Measured at seven accounts: each row is ~79px, because the row's two actions are **stacked** —
`Grant invoke` as a button with `Try it` beneath it as `text-xs underline text-muted-foreground`.
Seven accounts fill a 1000px viewport.

1. **Put the row's actions beside each other**, not stacked. `Try it` is the action the section's
   own prose tells the reader to use, so it must not be the smallest thing in the row — but the
   Explore idiom keeps row actions compact, so do not turn it into a second primary button.
2. **The identity column truncates every email** (`svc.aaaaa@demo.service.…`) at `w-48`. Task 1
   frees 272px; spend some of it here. `title` already exists — a tooltip is a fallback, not a fix,
   on the page whose subject *is* which identity may invoke.
3. Keep the `Invoke` column's caveat where it is: it is in the `<th>` with an `aria-label` so the
   550-character accessible name does not precede every row.

- [ ] **Step 1: Write the failing tests**
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove the row height in a real browser at seven accounts**
- [ ] **Step 5: Commit**

---

### Task 3: Try it — stop clipping the thing the tab exists for

**Files:**
- Modify: `detail/TryIt.tsx`
- Test: `__tests__/TryIt.test.tsx` + a real-browser check

The pane holding **Parameter values** reports `clientHeight 237` / `scrollHeight 297` — the header
row is sliced and 60px is behind an inner scrollbar — while ~400px of viewport sits empty below the
section. A caller cannot see the parameters they are being asked to fill.

`react-resizable-panels` v4: **sizes are STRINGS** (a bare number means px) and the prop is
`orientation`, not `direction`. In jsdom its `Separator` wins the hit-test with 0×0 rects and steals
focus — stub it, or unrelated typing tests fail.

Decide whether this section needs a split panel at all. It has three parts — *Run as*, *Parameter
values*, *Result* — and only the last one grows unboundedly. A split made sense when Preview shared
the pattern; **say what you chose and why**, and if you keep the split, the top pane must size to its
content rather than to a fixed fraction.

- [ ] **Step 1: Write the failing tests**
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove in a real browser that no parameter row is clipped at 1600×1000**
- [ ] **Step 5: Commit**

---

### Task 4: Response and Versions — one table, grouping inside it

**Files:**
- Modify: `detail/ResponseSection.tsx`, `detail/Versions.tsx`
- Test: their `__tests__` + a real-browser check

Response stacks four sections — *Frozen columns* (a 2-column table), *Response transform* (a
paragraph and a checkbox), the `Setting | Details | Value` table, and *OpenAPI document* (a
paragraph and a button) — each with a flat `text-sm` label and its own shape. Config carries the
same amount of material as **one** table with group rows, and that is the difference the user sees.

Bring Response onto one table with group rows. **Two things resist it and need a ruling rather than
a forced fit:** the frozen-columns listing is a *read-only listing*, not a setting, and the OpenAPI
block is an *action*, not a setting — §4.2 of the conventions already records a reversal on exactly
that distinction, so read it before deciding. Whatever you choose, the page must end with one
visual rhythm, not four.

Versions is already close — one table plus one action. Give it the same heading treatment as its
siblings so the tab strip reads consistently.

- [ ] **Step 1: Write the failing tests**
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove it bites**
- [ ] **Step 5: Commit**

---

### Task 5: Live check

**Every defect that mattered in this programme was found in a browser.** The suite is at ~5019 tests
and was green over every finding above.

Deploy this branch (`just deploy-kind-oidc`), confirm the running build is **this branch** by
grepping the served bundle for a string only it has, and **prove your instrument with a control** —
a `0` has three times been the instrument rather than the truth in this programme (`strings` absent
from the container, a zsh-eaten `--include`, and minified identifiers).

Seed enough data that density is real: several folders, ≥7 service accounts, an endpoint with
parameters, and an endpoint with none.

Walk all six tabs in **light and dark**, and report what you **saw**:

1. Do all six now agree on width? Measure, do not eyeball.
2. Access at seven accounts — row height, and can you read a full service-account address?
3. Try it — is any parameter row clipped? Is the empty space below gone?
4. Response — does the page read as one rhythm or as four stacked blocks?
5. A **trashed** endpoint — the fieldset is disabled; every disclosure must still open.
6. A narrow viewport, since three of the five findings concern width.

Fix what the browser finds; a layout claim needs a real-browser test.

- [ ] **Step 1: Run every gate**
- [ ] **Step 2: Deploy, seed, and walk the tabs**
- [ ] **Step 3: Commit whatever the browser made you fix**
