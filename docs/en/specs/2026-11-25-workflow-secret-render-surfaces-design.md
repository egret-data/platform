# secret() at render surfaces — design (package 10a)

**Roadmap:** package **#10 "secret() everywhere"** splits into **10a** (this: `secret()` in the
in-process render surfaces — Log, `runIf`, inputs, triggers — resolved and masked) and **10b**
(`secret()` in script env, injected into the container securely — its own spec). #10 completes when
both merge. The three http tasks already resolve `secret()` (pulled forward in package #4).

**Dependencies:** the existing secret machinery — `runners::secrets::render_with_secrets` (collect→
fetch→resolve, 3-pass, records `Resolved::values`), `logs::mask`, `runners::env::RenderContext`'s
execution-wide `redactions`, and `secret_inputs` (the SECRET-typed input seal). 10a reuses all of it;
it invents no new masking primitive.

## 1. Goal

Lift the confinement that resolves `secret()` only in `kestra.*` plugin-task properties and the three
http tasks, so a `secret()` call in a **Log** `message:`, a task's **`runIf:`**, a flow **input**
value/default, and a **trigger** (Schedule/Webhook condition and `labels`) resolves to the real value
at render time and runs as Kestra does — while every value it hands out is masked in every persisted
log line and in a failed run's message, exactly as a SECRET input's value already is. This is the
"replaces Appendix B §8 item 3's confinement with Kestra's reach" half of roadmap #10 that stays
in-process; the script-env half is 10b.

## 2. What exists today — the confinement is one installed resolver

`runners::env::environment` installs a **refusing** `secret` stub (the `REFUSED` row in `functions`,
with the pod-spec reason). Every surface renders against it, so `{{ secret('X') }}` is an error at
Log, `runIf`, inputs and triggers. Only `runners::secrets::render_with_secrets` installs a **working**
`secret`, per render, over a copy of the environment (`env::render_once_with`), and only the
`kestra.*` plugin-task property path (`worker::handlers::kestra`) and the http handlers call it. Its
`Resolved::values` records every value handed out; the caller masks that attempt's log with
`logs::mask`.

The execution-wide masking channel already exists and is load-bearing for SECRET inputs:
`RenderContext::redact(values)` feeds the redaction set that is applied to **every log line a run
writes** and to a failed run's message (via `RenderContext::redactions()` + `logs::mask`).
`secret_inputs::split` replaces a `type: SECRET` input value with `MASK`, seals the plaintext
(AES-256-GCM under `secrets.master_key`) into `executions.secret_inputs`, and `reveal`
(at `worker::render::Surface::load`) opens it so `{{ inputs.x }}` renders the real value — which is
then added to the redactions (`worker::render` calls `cx.redact(&self.secrets)`).

`secret_inputs.rs` records the posture that bounds 10a: **a `secret()` function's values are NOT
masked in a task's `outputs`** (W5b §12, as Kestra) — the author chose to publish it. So the
KV-value and output-`value:` surfaces stay refused / unmasked as a security divergence (§3.1); 10a
does not touch them.

So the confinement is not a separate subsystem to build — it is `render_with_secrets` being wired to
four more render paths, each then feeding its `Resolved::values` into the execution's `redactions`.

## 3. Kestra measurement — Task 0, load-bearing

This package banks **0 corpus flows**, so there is **no corpus-ratchet coupling** (unlike packages 9a
and 9b): the census tuple, the driver's `LIVE`/`VALIDATE_REFUSED` constants, and the
`PublishedButRefused` table are not moved by 10a. The measurement is on the Kestra source (pin
`96893b589`, 2.0.0-SNAPSHOT) and the feature surface, not the golden. Task 0 measures and records in
§6, and no test asserts a terminal until egret reproduces what is measured:

- **M1 — masking reach.** Does Kestra mask a `secret()` value in *every* log line of the execution, or
  only in the task that resolved it? (egret's `redactions` is execution-wide; confirm that matches.)
- **M2 — inputs posture (the open decision).** When a flow **input** value/default renders to include
  a `secret()`, does Kestra store the plaintext in the execution's inputs, or keep it out (mask/seal)?
  This decides whether 10a routes such a value through `secret_inputs::split` (seal, the safe parity
  choice) or only adds it to `redactions`. **Decision deferred to this measurement** (user, 2026-11-25).
- **M3 — trigger render timing.** At what point are a Schedule/Webhook trigger's `secret()` calls
  resolved (trigger evaluation vs. execution start), and are their values masked in the trigger's own
  logs?
- **M4 — runIf.** Confirm `secret()` in `runIf` resolves to a boolean and never surfaces the value
  (it should only reach a log via an error message, which `redactions` covers).

## 4. Decisions

- **D1 — reuse, not reinvent.** 10a installs `render_with_secrets` (or the same resolver seam) on the
  Log, `runIf`, input and trigger render paths; it adds each `Resolved::values` to the execution's
  `RenderContext` redactions. No new masking primitive, no migration, no API, no model field.
- **D2 — masking is execution-wide and mandatory.** A value a `secret()` hands out at any 10a surface
  is masked in every persisted log line and in a failed run's message, via the redactions — the same
  channel SECRET inputs use. A surface is not lifted until its values reach the redactions.
- **D3 — inputs seal posture decided by M2.** If Kestra keeps input `secret()` plaintext out of the
  stored execution, 10a seals it through `secret_inputs` (reuse `split`/`reveal`); otherwise it only
  adds the value to the redactions. Recorded in §6 after Task 0.
- **D4 — KV value and output `value:` stay refused/unmasked (§3.1).** Not in scope; their refusals are
  not lifted. This is the deliberate security divergence `secret_inputs.rs` already documents.
- **D5 — the `WF_USE_SECRETS` gate is unchanged.** `routes::source_uses_secret` already refuses a
  publish/start by a caller lacking `workflow:use_secrets` whenever the source can call `secret()`.
  Widening where `secret()` *resolves* does not widen where it is *detected*; the gate already covers
  every surface because it scans the source text. Confirm no 10a surface escapes `source_uses_secret`.
- **D6 — lift last, after the behavior is correct, grep every encoding.** The lift-refusal lesson bit
  package 9a twice and 9b once (the latest, 9b, was a hardcoded fixture YAML in a test's "still
  refused" list that neither a code grep nor a message grep caught): before removing a refusal, grep
  its **code**, its **message text**, AND any **fixture YAML** that asserts it, across `src/ tests/
  oracle-exec/`. The grep is the authority; §5 is a starting map, not a substitute.

## 5. Lift blast radius (grep is the authority)

Every place that encodes a "secret() is refused here" for a 10a surface must change when that surface
is lifted. Known starting points (Task 0's grep confirms the full set on the worktree):

- `src/spec/validate.rs` — the secret-refused arms for Log/runIf/inputs/triggers + their unit tests +
  the module header note.
- `src/runners/secrets.rs` — the header prose ("script tasks, inputs, `runIf`, `Log` and triggers all
  render against [the refusing stub]") and the `REFUSED` row wiring for these surfaces.
- `src/plugins/mod.rs` — the `secret()` placement refusal (keep the parts that still apply to non-10a
  surfaces; lift only the 10a ones).
- `src/eval.rs` — the eval endpoint keeps `secret()` sealed (it is a read surface, not execution);
  confirm 10a does not change the eval seal.
- Any fixture-keyed test (routes/publish/worker) that uses a Log/runIf/input/trigger `secret()` flow
  as a "must be refused" case — the 9a/9b 6th/7th-file analog. Grep for the fixtures, not just codes.

## 6. Differences / measured facts (Task 0 — fill from Kestra before any terminal is pinned)

*(Empty until Task 0 runs. Record the M1–M4 answers, the inputs seal decision (D3), and any observed
divergence here, with `file:line` into the Kestra checkout `data-portal/kestra` @ `96893b589`.)*

## 7. Scope

**In:** resolve `secret()` at Log `message:`, task `runIf:`, flow input value/default, and
Schedule/Webhook triggers; mask every handed-out value execution-wide via the redactions; lift the
matching refusals; the inputs seal posture per M2; tests; docs.

**Out:** `secret()` in script/container **env** (→ 10b); the Kubernetes runner (→ #17); `secret()` in
**KV value** and task **output `value:`** (§3.1 security divergence, unchanged); the eval endpoint seal
(unchanged); any new secret *source* or *store* (the workspace secret store is unchanged).

## 8. Testing

- **Per surface** (Log, runIf, inputs, triggers): a flow whose `secret('X')` resolves there runs, and
  the secret value does **not** appear in any persisted log line or in the stored execution
  (history/inputs). **Mutation:** neutering the redaction feed for that surface must turn the test red
  (the assertion is on absence-of-plaintext, which a disabled masker fails).
- **Controls that stay refused:** a `secret()` in a KV value and in an output `value:` are still
  refused/unmasked (D4).
- **Gate:** a flow calling `secret()` at any 10a surface is still refused for a caller without
  `workflow:use_secrets` (D5).
- **Scoped runs must include `--test routes --test execution_timestamps`** (project rule) and
  `--test executor` to confirm the census tuple is UNCHANGED (banks 0).

## 9. Risk

Low–medium. The machinery (resolver + execution-wide masker + input seal) already exists and is
proven for SECRET inputs and plugin-task `secret()`; 10a wires it to four more paths. The real risks
are (a) a surface lifted before its values reach the redactions (D2 guards this — test on
absence-of-plaintext with a mutation), and (b) a missed refusal encoding at lift (D6 — grep every
encoding, including fixtures, the 9a/9b lesson). No corpus-ratchet coupling (banks 0), so the census
cannot go red mid-suite — the one simplification 10a has over 9a/9b.
