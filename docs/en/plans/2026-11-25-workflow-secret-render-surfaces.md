# secret() at render surfaces (package 10a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve `secret()` in Log `message:`, task `runIf:`, flow inputs, and triggers, masking every
value it hands out execution-wide — reusing the existing resolver + masker, inventing nothing.

**Architecture:** The confinement is one installed resolver plus one validate-time walker.
`render_with_secrets(props: &Map<String,Value>, cx, workspace, source) -> Result<(Map, Resolved), String>`
(`crates/egret-workflow/src/runners/secrets.rs:141`, **async, one pass**) is called today only by the
`kestra.*` plugin handler (`src/worker/handlers/kestra.rs:718`) and the http handlers. `Resolved.values`
is a **public `Vec<String>` field** (no accessor). Every other surface renders against the refusing
`secret` stub, and `validate.rs`'s `Walk::outside` refuses `secret()` in their source at publish. 10a,
per surface: (1) adds that surface's path shape to the walker allow-list; (2) renders it with the
resolver; (3) masks the surface's OWN attempt lines + failure message with the resolved values via the
attempt's `RenderContext::redact(&[String])` (`src/runners/env.rs:413`) — the per-attempt channel that
already masks SECRET inputs (`src/worker/claim.rs:272` builds the masked log from
`task.render.redactions()`). **Masking is per-attempt/render, NOT execution-wide** (Kestra masks
per-logger — Task 0 §6.1); the only execution-wide persistence is the SECRET-input seal (`secret_inputs`),
which Task 3 extends. **runIf and inputs render SYNCHRONOUSLY inside settle passes** (the resolver is
async), so those tasks pre-fetch the secrets (the collect/fetch/resolve split) before the sync pass.

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
- Banks **0 corpus flows — ONLY IF `triggers.*.key` stays refused** (Task 0 §6.6). The census tuple,
  driver `LIVE`/`VALIDATE_REFUSED`, and `PublishedButRefused` MUST NOT move. `valids/webhook-secret-key`
  (Webhook `key:`) sits in `VALIDATE_REFUSED`; a lift that allows `secret()` under `triggers.*.key`
  moves it and trips the census. Keep `key` refused (Task 4). If any census number changes, STOP and
  reconcile.
- **Lift = a path ALLOW-LIST, not arm removal (Task 0 §6.6).** The refusal is one walker,
  `secret_outside_plugin_task` (`src/spec/validate.rs` `Walk::outside`, ~614), that refuses `secret()`
  in EVERY string outside a `kestra.*`/http task's properties. Each task lifts ONE path shape by adding
  it to the walker's allow-list; it never deletes the walker. Stays refused after 10a: `Return.format`,
  outputs `value:`, Set/Put `value`/`key`, script `commands`, Webhook `key`, nested `render()`, the eval
  seal.
- **Lift-refusal discipline (bit 9a twice, 9b once):** before changing the walker, grep its **code**
  (`CODE_SECRET_OUTSIDE_PLUGIN_TASK`), its **message** (`functions::SECRET_REFUSAL`, ~`functions.rs:323`
  — its text becomes false and must be reworded), AND any **fixture YAML** asserting it, across `src/
  tests/ oracle-exec/`. Asserting tests to update: `tests/plugins_gate.rs:1027-1110`,
  `src/runners/secrets.rs:997/1026/1085/1102`, `tests/publish.rs:917-960`, `src/spec/validate.rs:6013-6027`.
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
- Modify: the Log handler `worker::handlers::log::Log::run` (`src/worker/handlers/log.rs:91`, today
  `render_str(text, &task.render)`). Unlike the kestra/http handlers it holds **no `SecretSource`**
  (they hold `rt.secrets`, `kestra.rs:204`/`http.rs:200`) — Task 1 must give the Log handler the secret
  source, render `message:` with `render_with_secrets`, then `cx.redact(&resolved.values)` on the
  attempt's render context so the masked log (built from `task.render.redactions()`, `claim.rs:272`)
  hides it.
- Modify: `src/spec/validate.rs` `Walk::outside` — **add `tasks.*.message` (Log) to the allow-list**
  (do NOT delete the walker) + update its unit test (`validate.rs:6013-6027` asserts `tasks.l.message`
  refused). Reword `functions::SECRET_REFUSAL` once (shared; Task 5 finalises).
- Modify: `src/runners/secrets.rs` header prose (remove `Log` from the "render against the refusing stub"
  list).
- Test: a new test proving the Log flow runs and the plaintext is absent.

**Interfaces:**
- Consumes (confirmed in Task 0): `render_with_secrets(props, cx, workspace, source) -> Result<(Map,
  Resolved), String>` (async); `Resolved.values` (public `Vec<String>` field); `RenderContext::redact(&[String])`.
- Produces: a Log message render that resolves `secret()` and masks its values on the attempt. The
  reworded `SECRET_REFUSAL` text (Tasks 2–5 build on it).

- [ ] **Step 1: Write the failing test.** A flow with a Log task whose `message:` is
  `"pw={{ secret('DB_PASSWORD') }}"`, a workspace secret `DB_PASSWORD=hunter2-SENTINEL`, the caller
  holding `WF_USE_SECRETS`. Drive it; assert the task succeeds AND no persisted log line and no stored
  history contains `hunter2-SENTINEL` (assert the masked form `pw=******` appears instead).
- [ ] **Step 2: Run it — expect FAIL** (today the Log render refuses `secret()` → the flow is refused or
  the render errors).
- [ ] **Step 3: Wire the resolver.** In the Log handler, replace the plain render of `message:` with
  `render_with_secrets`, then `cx.redact(resolved.values)` before writing the log line (mirror
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
- Modify: `worker::render::render_run_if` (`src/worker/render.rs:1763`, **synchronous** `render_str`,
  called from `src/worker/claim.rs:5605` inside a settle pass). A sync render cannot await a secret
  fetch, so Task 2 adds a **pre-fetch** before the settle pass: collect the `secret()` keys the runIf
  needs (the resolver's collect phase / `literal_keys`), fetch them async from the `SecretSource`, then
  render the runIf synchronously with the resolved values in hand, and `redact` those values on the
  attempt's render context (a runIf failure logs its error on the task's logger).
- Modify: `src/spec/validate.rs` `Walk::outside` — **add `tasks.*.runIf` to the allow-list** (+ the
  `validate.rs:6013` test that asserts `tasks.r.runIf` refused).
- Modify: `src/runners/secrets.rs` header prose (remove `runIf`).
- Test: a flow whose `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` gates a task.

**Interfaces:**
- Consumes: the resolver's collect/fetch halves (sync render needs pre-fetched values) + `redact`.
- Produces: a runIf evaluation that resolves `secret()` to a boolean and masks the values on its attempt.

- [ ] **Step 1: Failing test.** Secret `FLAG=on-SENTINEL-7f`; a task with
  `runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"` runs, a sibling with a mismatched compare is
  SKIPPED; assert no persisted log/message/history contains `on-SENTINEL-7f`.
- [ ] **Step 2: Run — expect FAIL** (runIf refuses `secret()` today).
- [ ] **Step 3: Wire the pre-fetch + resolver** at the runIf seam (collect keys → async fetch before the
  settle pass → sync render with resolved values → redact); add `tasks.*.runIf` to the walker allow-list.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Mutation check.** Drop the redact; a secret value that reaches an error message must then
  leak the test red. If runIf never logs the value on the happy path, add a failing-render case that
  surfaces it through the error message so the mutation is observable.
- [ ] **Step 6: Scoped bar** (incl. `--test executor` census-unchanged check).
- [ ] **Step 7: Commit** (`feat(workflow): resolve secret() in runIf, masked execution-wide`).

---

### Task 3: secret() in flow inputs (D3 = SEAL — stricter than Kestra, user-confirmed 2026-11-25)

**D3 is decided: SEAL.** Kestra stores the plaintext for non-SECRET input types; egret seals it instead
(a deliberate, stricter security divergence — the class `secret_inputs` exists to keep secrets out of
`executions.inputs` and the API body). Two facts from Task 0 §6.2 shape this task:
1. **egret never renders a templated default at start today** — `worker::render::with_declared_defaults`
   (`src/worker/render.rs:1036`) inserts the **raw template source**, which only resolves later because
   `render_str` re-renders its own output (multi-pass). So a `{{ inputs.x }}` whose default is
   `{{ secret('X') }}` would NOT resolve through the one-pass resolver. Task 3 must render the templated
   default **at start, with the resolver**, and seal the resulting value — as Kestra renders it at start.
2. **`secret_inputs::split` seals ids declared `type: SECRET` only** (`src/secret_inputs.rs:68-80`, via
   `secret_ids`). Task 3 must let `split` (or a sibling) take an **extra id-set**: the non-SECRET inputs
   whose rendered value resolved a secret, so they are sealed too.

**Files:**
- Modify: the inputs render-at-start seam — `worker::claim::checked_inputs`/`checked_inputs_for`/
  `input_renderer` (`src/worker/claim.rs:4279-4330`) → `inputs_check::resolve` (`src/inputs_check.rs`),
  which is **synchronous, no secret source** → add a **pre-fetch** (like Task 2) so the start render has
  the secret values; then seal the resolved value via `secret_inputs::split` taking the id-set.
- Modify: `src/secret_inputs.rs` — `split` (or a sibling) to accept an extra id-set to seal.
- Modify: `src/spec/validate.rs` `Walk::outside` — **add `inputs.*.defaults` to the allow-list** (+ its
  test; `plugins_gate.rs:1027-1110` has an `inputs.i.defaults` case).
- Modify: `src/runners/secrets.rs` header prose (remove `inputs`).
- Test: a flow input whose value/default is `{{ secret('TOKEN') }}`.

**Interfaces:**
- Consumes: the resolver's collect/fetch halves (sync start render) + `secret_inputs::split(inputs,
  declaration, extra_seal_ids)` and `reveal` at `Surface::load` (`src/worker/render.rs:230`).
- Produces: an input whose rendered `secret()` value is **sealed** (never plaintext in
  `executions.inputs`) and masked in logs.

- [ ] **Step 1: Failing test.** Secret `TOKEN=tok-SENTINEL-9a`; a flow input default `{{ secret('TOKEN') }}`
  consumed by a downstream `{{ inputs.x }}`; assert `executions.inputs` holds `MASK` (not the plaintext),
  the downstream task renders the real value, and no log line leaks `tok-SENTINEL-9a`.
- [ ] **Step 2: Run — expect FAIL.**
- [ ] **Step 3: Wire** the pre-fetch + start render of the templated default with the resolver; seal the
  resolved value through `secret_inputs::split` with the id-set; add `inputs.*.defaults` to the walker
  allow-list.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Mutation check** (drop the seal → the plaintext appears in `executions.inputs` → test red;
  drop the redact → it appears in a log → test red).
- [ ] **Step 6: Scoped bar** (incl. census-unchanged).
- [ ] **Step 7: Commit** (`feat(workflow): resolve and seal secret() in flow inputs`).

---

### Task 4: secret() in triggers (Webhook inputs + Flow-trigger when/inputs — NOT Schedule)

**Scope correction (Task 0 §6.5/§6.7):** egret has **no Schedule `when`** (refused, roadmap #11), no
Webhook `conditions` render, and refuses a templated Schedule `inputs`. The only render seams in 10a are
**Webhook `inputs`** and **Flow-trigger `when`/`inputs`**. **Webhook `key:` STAYS REFUSED** — it is a
credential compared per request, and lifting it would move `valids/webhook-secret-key` out of
`VALIDATE_REFUSED` and trip the census (banks-0 depends on this).

**Files:**
- Modify: `trigger::webhook::render_inputs` (`src/trigger/webhook.rs:1007`, called at
  `src/webhook_routes.rs:645`; **synchronous, no source** → pre-fetch like Task 2) and the Flow-trigger
  `when`/`inputs` render (`src/flow_trigger/process.rs:955` `render_when`, `:996` `start_fire`). Do NOT
  touch `trigger::webhook::render_key` (`webhook.rs:540`) — `key` stays refused.
- Modify: `src/spec/validate.rs` `Walk::outside` — add ONLY `triggers.*.inputs` and the Flow-trigger
  `when`/`inputs` paths to the allow-list; leave `triggers.*.key` refused (its `plugins_gate.rs` case
  `triggers.w.key` must still assert refused).
- Modify: `src/runners/secrets.rs` header prose (narrow `triggers` to the lifted paths).
- Test: a Webhook `inputs` and a Flow-trigger `when` using `{{ secret('X') }}`; plus a control that
  Webhook `key` is still refused.

**Interfaces:**
- Consumes: the resolver's collect/fetch halves (sync trigger render) + `redact` on the trigger's logger.
- Produces: Webhook `inputs` and Flow-trigger `when`/`inputs` that resolve `secret()`, masked in the
  trigger's own logs; `triggers.*.key` still refused.

- [ ] **Step 1: Failing test** pinned to M3 timing: a Webhook `inputs` value and a Flow-trigger `when`
  using `secret('X')` (value `trg-SENTINEL-3c`) resolve, and the value is absent from the trigger's
  persisted logs. Plus a control asserting a Webhook `key:` with `secret()` is STILL refused.
- [ ] **Step 2: Run — expect FAIL.**
- [ ] **Step 3: Wire** the pre-fetch + resolver at the two seams; add only the inputs/when paths to the
  walker allow-list, leaving `key` refused.
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
  `LIVE`, `VALIDATE_REFUSED`, `PublishedButRefused` are all unchanged from pre-10a. This holds ONLY
  because `triggers.*.key` stayed refused, so `valids/webhook-secret-key` is still in `VALIDATE_REFUSED`
  and `valids/secrets.yaml` still refused on its `Return.format` — confirm both explicitly.
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
- **Task 0 corrections folded in:** D2 masking is per-attempt (not execution-wide); D3 = SEAL (decided);
  lifts are walker allow-lists (not arm removals); triggers = Webhook inputs + Flow-trigger when/inputs
  with `key` kept refused; runIf/inputs need a pre-fetch; `Resolved.values` is a field. The seam
  file:line are from Task 0 §6.5 on this worktree.
- **Type consistency:** `render_with_secrets(...) -> Result<(Map, Resolved), String>`, `Resolved.values`
  (field), `RenderContext::redact(&[String])`, `secret_inputs::split` (extended to take an extra
  id-set) — used consistently across Tasks 1–4.
