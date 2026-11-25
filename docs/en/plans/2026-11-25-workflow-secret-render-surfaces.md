# secret() at render surfaces (package 10a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve `secret()` in Log `message:`, task `runIf:`, flow inputs, and triggers, masking every
value it hands out execution-wide — reusing the existing resolver + masker, inventing nothing.

**Architecture:** The confinement is one installed resolver. `render_with_secrets(props, cx, workspace,
source) -> (Value, Resolved)` (`crates/egret-workflow/src/runners/secrets.rs`) is called today only by
the `kestra.*` plugin handler (`src/worker/handlers/kestra.rs`). Every other surface renders against the
refusing `secret` stub. 10a calls the same resolver on four more render paths and feeds each
`Resolved::values()` into the execution's redaction set via `RenderContext::redact(values)`
(`src/runners/env.rs`), the same channel SECRET inputs already use (`src/worker/render.rs`,
`cx.redact(&self.secrets)`). The masker `logs::mask(text, cx.redactions())` and the AES seal
`secret_inputs` already exist.

**Tech Stack:** Rust, `egret-workflow` crate; minijinja render; existing `runners::secrets`,
`runners::env::RenderContext`, `logs::mask`, `secret_inputs` modules. No migration, no API, no model field.

**Spec:** `docs/en/specs/2026-11-25-workflow-secret-render-surfaces-design.md` (VI beside it). Exact seam
locations are confirmed by Task 0 on this worktree (function names are stable; line numbers are not —
Task 0 re-locates every seam before Tasks 1–4 touch it).

## Global Constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` only in PR body.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends
  `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage by explicit path; never `git add -A`; never stage `target*/`. Never commit secrets/scratch.
- ONE heavy Rust build at a time; builds run FOREGROUND in subagents. `cargo fmt --all --check` before push.
- Scoped `egret-workflow` test runs MUST include `--test routes --test execution_timestamps`.
- Banks **0 corpus flows**: the census tuple, driver `LIVE`/`VALIDATE_REFUSED`, and `PublishedButRefused`
  MUST NOT move. If any of them changes, a surface was lifted that a corpus flow exercises — STOP and
  reconcile (this would make 10a corpus-coupled like 9a/9b, which the measurement says it is not).
- **Lift-refusal discipline (bit 9a twice, 9b once):** before removing any refusal, grep its **code**,
  its **message text**, AND any **fixture YAML** asserting it, across `src/ tests/ oracle-exec/`.
- **Measurement-first:** no test asserts a Kestra-behavior terminal until Task 0 records it in spec §6.
- DB: a worktree-local postgres + valkey (set both `DATABASE_URL` and `EGRET_DATABASE__URL`). Scoped
  `--test X` runs locally; the full `-p egret-workflow` run hangs — use CI for the whole suite.

---

### Task 0: Measure Kestra, locate seams, map the lift blast radius

**Files:**
- Modify (record findings): spec §6 of `docs/{en,vi}/specs/2026-11-25-workflow-secret-render-surfaces-design.md`
- Read-only: Kestra checkout `data-portal/kestra` @ `96893b589`; `src/runners/secrets.rs`,
  `src/runners/env.rs`, `src/worker/render.rs`, `src/worker/handlers/`, `src/spec/validate.rs`,
  `src/secret_inputs.rs`, the Log handler, the runIf eval seam, the start/inputs render seam, the
  trigger eval seam.

**Interfaces:**
- Produces: spec §6 filled (M1–M4 + D3 decision); the four render seams located by function on this
  worktree; the full lift blast-radius grep. Tasks 1–5 consume these.

**Deliverable:** spec §6 filled; the four render seams located by function; the lift blast-radius grep
captured. No production code changes. This task gates every later task.

- [ ] **Step 1: M1 masking reach.** In the Kestra checkout, confirm a `secret()` value is masked in every
  log line of the execution (not only the resolving task). Record the class(es) that implement it
  (`file:line`) in spec §6.
- [ ] **Step 2: M2 inputs posture (THE open decision).** Measure: when a flow input value/default renders
  to a `secret()`, does Kestra store the plaintext in the execution's inputs or keep it out? Record the
  answer and set **D3** in §6: seal through `secret_inputs::split` (if kept out) or redaction-only (if
  stored). Task 3 reads this decision.
- [ ] **Step 3: M3 trigger timing.** Record when Schedule/Webhook `secret()` calls resolve and whether
  their values are masked in the trigger's own logs.
- [ ] **Step 4: M4 runIf.** Confirm `secret()` in `runIf` yields a boolean and never surfaces the value
  except through an error message.
- [ ] **Step 5: Seam map.** Record the exact function + file for each surface's render on THIS worktree:
  Log handler message render; runIf evaluation; inputs render at start; trigger condition/labels render.
  Verify `render_with_secrets`'s exact signature and `Resolved`'s value accessor name here.
- [ ] **Step 6: Lift map.** `grep -rn` across `src/ tests/ oracle-exec/` for each secret-refusal's code,
  message text, and fixture YAML touching Log/runIf/inputs/triggers. Capture the full list; this is the
  authority for Tasks 1–4's lifts. Confirm `source_uses_secret` covers every 10a surface (D5).
- [ ] **Step 7: Commit** the spec §6 update
  (`docs(workflow): record Kestra secret() render-surface measurements (10a Task 0)`).

---

### Task 1: secret() in Log `message:`

**Files:**
- Modify: the Log handler (`core.log.Log`; locate in `src/worker/handlers/` — Task 0 seam map) to render
  `message:` with `render_with_secrets` and `cx.redact(resolved.values())` before the line is written.
- Modify: `src/spec/validate.rs` — lift the Log secret-refusal arm (+ its unit test, + header note).
- Modify: `src/runners/secrets.rs` header prose (remove `Log` from the "render against the refusing stub"
  list).
- Test: a new test proving the Log flow runs and the plaintext is absent.

**Interfaces:**
- Consumes (confirmed in Task 0): `render_with_secrets(props, cx, workspace, source) -> (Value, Resolved)`;
  `Resolved::values()`; `RenderContext::redact(&[String])`.
- Produces: a Log message render that resolves `secret()` and contributes its values to the redactions.

- [ ] **Step 1: Write the failing test.** A flow with a Log task whose `message:` is
  `"pw={{ secret('DB_PASSWORD') }}"`, a workspace secret `DB_PASSWORD=hunter2-SENTINEL`, the caller
  holding `WF_USE_SECRETS`. Drive it; assert the task succeeds AND no persisted log line and no stored
  history contains `hunter2-SENTINEL` (assert the masked form `pw=******` appears instead).
- [ ] **Step 2: Run it — expect FAIL** (today the Log render refuses `secret()` → the flow is refused or
  the render errors).
- [ ] **Step 3: Wire the resolver.** In the Log handler, replace the plain render of `message:` with
  `render_with_secrets`, then `cx.redact(resolved.values())` before writing the log line (mirror
  `handlers/kestra.rs`). Lift the Log refusal arm in `validate.rs` (use the Task 0 lift map — code AND
  message AND fixtures).
- [ ] **Step 4: Run it — expect PASS.**
- [ ] **Step 5: Mutation check.** Temporarily drop the `cx.redact(...)` call; the test must go red
  (plaintext now leaks). Restore it. Record that the test is mutation-sensitive.
- [ ] **Step 6: Run the scoped bar** `cargo test -p egret-workflow --test <this> --test routes --test
  execution_timestamps --test executor` (and confirm the census tuple is UNCHANGED — banks 0).
- [ ] **Step 7: Commit** (`feat(workflow): resolve secret() in Log message, masked execution-wide`).

---

### Task 2: secret() in task `runIf:`

**Files:**
- Modify: the `runIf` evaluation seam (Task 0 seam map) to render with `render_with_secrets` and
  `cx.redact(resolved.values())`.
- Modify: `src/spec/validate.rs` — lift the runIf secret-refusal arm (+ test + header).
- Modify: `src/runners/secrets.rs` header prose (remove `runIf`).
- Test: a flow whose `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` gates a task.

**Interfaces:**
- Consumes: same resolver + `redact` as Task 1.
- Produces: a runIf evaluation that resolves `secret()` to a boolean and contributes values to the
  redactions.

- [ ] **Step 1: Failing test.** Secret `FLAG=on-SENTINEL-7f`; a task with
  `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` runs, a sibling with a mismatched compare is
  SKIPPED; assert no persisted log/message/history contains `on-SENTINEL-7f`.
- [ ] **Step 2: Run — expect FAIL** (runIf refuses `secret()` today).
- [ ] **Step 3: Wire the resolver** at the runIf seam; lift the runIf refusal (Task 0 lift map).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Mutation check.** Drop the redact; a secret value that reaches an error message must then
  leak the test red. If runIf never logs the value on the happy path, add a failing-render case that
  surfaces it through the error message so the mutation is observable.
- [ ] **Step 6: Scoped bar** (incl. `--test executor` census-unchanged check).
- [ ] **Step 7: Commit** (`feat(workflow): resolve secret() in runIf, masked execution-wide`).

---

### Task 3: secret() in flow inputs (posture per Task 0 D3)

**Files:**
- Modify: the inputs render-at-start seam (Task 0 seam map — the start path in `src/routes.rs` /
  `src/secret_inputs.rs` / `src/worker/render.rs`).
- Modify: `src/spec/validate.rs` — lift the inputs secret-refusal arm (+ test + header).
- Modify: `src/runners/secrets.rs` header prose (remove `inputs`).
- Test: a flow input whose value/default is `{{ secret('TOKEN') }}`.

**Interfaces:**
- Consumes: resolver + `redact`; **if D3 = seal:** `secret_inputs::split(inputs, declaration) ->
  (masked, sealed)` and `reveal` at `Surface::load`.
- Produces: an input whose rendered `secret()` value is masked everywhere (and, if D3 = seal, never
  stored plaintext in `executions.inputs`).

- [ ] **Step 1: Read D3 from spec §6** (set by Task 0). This task's exact assertion depends on it:
  seal → assert `executions.inputs` holds `MASK`, not plaintext; redact-only → assert the value is
  masked in logs but may be stored (matching Kestra).
- [ ] **Step 2: Failing test.** Secret `TOKEN=tok-SENTINEL-9a`; a flow input value `{{ secret('TOKEN') }}`
  consumed by a downstream `{{ inputs.x }}`; assert per D3 and that no log line leaks the plaintext.
- [ ] **Step 3: Run — expect FAIL.**
- [ ] **Step 4: Wire** the resolver at the inputs seam; if D3 = seal, route the resolved value through
  `secret_inputs::split` so it is sealed like a SECRET input; add values to the redactions either way.
  Lift the inputs refusal (Task 0 lift map).
- [ ] **Step 5: Run — expect PASS.**
- [ ] **Step 6: Mutation check** (drop the seal/redact per D3 → red).
- [ ] **Step 7: Scoped bar** (incl. census-unchanged).
- [ ] **Step 8: Commit** (`feat(workflow): resolve secret() in flow inputs, masked execution-wide`).

---

### Task 4: secret() in triggers (Schedule / Webhook)

**Files:**
- Modify: the trigger condition/labels render seam (Task 0 seam map — the trigger eval; `webhook_routes.rs`
  for Webhook).
- Modify: `src/spec/validate.rs` — lift the trigger secret-refusal arm (+ test + header).
- Modify: `src/runners/secrets.rs` header prose (remove `triggers`).
- Test: a Schedule/Webhook trigger using `{{ secret('X') }}`.

**Interfaces:**
- Consumes: resolver + `redact`, applied at trigger-eval time (M3 timing).
- Produces: triggers that resolve `secret()` with their values masked in the trigger's logs.

- [ ] **Step 1: Failing test** pinned to M3's measured timing: a trigger whose condition/labels use
  `secret('X')` (value `trg-SENTINEL-3c`) resolves, and the value is absent from the trigger's persisted
  logs.
- [ ] **Step 2: Run — expect FAIL.**
- [ ] **Step 3: Wire** the resolver at the trigger seam; lift the trigger refusal (Task 0 lift map).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Mutation check.**
- [ ] **Step 6: Scoped bar** (incl. census-unchanged + `--test routes`, since triggers touch routes).
- [ ] **Step 7: Commit** (`feat(workflow): resolve secret() in triggers, masked execution-wide`).

---

### Task 5: Reconcile, docs, final grep

**Files:**
- Modify: `src/runners/secrets.rs` module header (now describes the four lifted surfaces + the two that
  stay confined: KV value, output `value:`).
- Modify: docs-repo `egret-data/docs` `platform/workflows/flows/expressions.md` (and the error-handling /
  flow-control page if it mentions the secret() confinement): `secret()` now works in Log, runIf, inputs
  and triggers, with masking; still refused in KV/output value.
- Test: whole-surface grep + the still-refused controls.

- [ ] **Step 1: Grep clean.** Across `src/ tests/ oracle-exec/`, confirm no lifted surface's refusal
  code/message/fixture remains, and the two CONFINED surfaces (KV value, output `value:`) are STILL
  refused (keep/verify their controls — the 9a/9b "6th/7th file" discipline).
- [ ] **Step 2: Census-unchanged proof.** `cargo test -p egret-workflow --test executor` — the tuple,
  `LIVE`, `VALIDATE_REFUSED`, `PublishedButRefused` are all unchanged from pre-10a (banks 0).
- [ ] **Step 3: Controls test.** A `secret()` in a KV value and in an output `value:` are still refused
  (one test each, asserting the specific still-live refusal codes).
- [ ] **Step 4: Gate test.** A flow calling `secret()` at each lifted surface is still refused for a
  caller without `workflow:use_secrets` (D5).
- [ ] **Step 5: Docs** update (EN + VI) in the docs-repo on a `docs/` branch; its own `build` gate green.
- [ ] **Step 6: Full scoped bar** `cargo fmt --all --check` + `cargo clippy` + the scoped tests + docs
  build. Commit (`feat(workflow): finish secret() render-surface reach (10a) + docs`).

---

## Self-Review

- **Spec coverage:** §1 goal → Tasks 1–4 (one surface each). §3 M1–M4 → Task 0. §4 D1/D2 → the
  resolver+redact wiring in each task. D3 → Task 0 + Task 3. D4 (KV/output stay refused) → Task 5
  controls. D5 (gate) → Task 0 Step 6 + Task 5 Step 4. D6 (lift discipline) → Task 0 lift map + each
  task's lift step + Task 5 grep. §8 testing (absence-of-plaintext + mutation) → each task's Steps. §9
  risks (lift-before-redact; missed refusal) → mutation checks + Task 0/Task 5 grep. No gap.
- **Placeholder scan:** the only deferred value is D3 (inputs seal vs redact), a named measurement with a
  decider (Task 0) and a conditional in Task 3 — the measurement-first discipline the spec mandates, not
  a "TBD". Seam line numbers are deliberately omitted; Task 0 locates them on this worktree.
- **Type consistency:** `render_with_secrets(...) -> (Value, Resolved)`, `Resolved::values()`,
  `RenderContext::redact(&[String])`, `secret_inputs::split -> (masked, sealed)` — used consistently
  across Tasks 1–4; Task 0 Step 5 confirms the exact accessor/signature on this worktree.
