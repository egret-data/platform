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
- **D2 — masking is per-attempt and mandatory (corrected by Task 0 §6.1).** Kestra masks per-logger,
  not execution-wide; egret already redacts each attempt from `task.render.redactions()`. So a value a
  `secret()` hands out at a 10a surface is masked in that surface's OWN attempt lines and its OWN failure
  message. The only execution-wide persistence is the SECRET-input seal (D3). A surface is not lifted
  until its resolved values reach its attempt's redactions. (Also: lifts are allow-list entries on the
  `secret_outside_plugin_task` walker, not arm removals; runIf and inputs render synchronously inside
  settle passes, so those surfaces pre-fetch the secrets before the pass — §6.5.)
- **D3 — inputs: SEAL (user-confirmed 2026-11-25; stricter than Kestra).** Task 0 §6.2 measured that
  Kestra STORES the plaintext for non-SECRET input types. egret instead seals a resolved input secret
  through `secret_inputs::split` (extended to take an extra id-set), as it does a `type: SECRET` input —
  a deliberate, stricter security divergence, because parity would put the secret in `executions.inputs`
  and the API body in clear. This requires rendering a templated default **at start** (egret does not
  today — `with_declared_defaults` inserts the raw template).
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

## 6. Differences / measured facts (Task 0, measured 2026-11-25)

Source: Kestra checkout `data-portal/kestra` @ `96893b589`, read (not run). Paths are under
`core/src/main/java/io/kestra/core/` unless prefixed.

### 6.1 M1 — masking reach: per-logger, NOT execution-wide

`secret()` calls `SecretFunction.consumeSecret` (`runners/pebble/functions/SecretFunction.java:121-128`),
which invokes the `addSecretConsumer` variable (`RunVariables.SECRET_CONSUMER_VARIABLE_NAME`,
`runners/RunVariables.java:30`). That consumer is `RunContextLogger::usedSecret`
(`runners/RunContextLogger.java:187-192`; wired at `RunVariables.java:567-569` and
`RunContextInitializer.java:144,238,265,280-281`). `usedSecret` adds the plaintext **and its base64** to
the logger's private `useSecrets` list; every appender replaces them with `******` on its own
logger's lines (`BaseAppender.replaceSecret`, `RunContextLogger.java:376-385`).

So masking is **per `RunContextLogger`** (one per task run / trigger), not per execution. A secret
resolved in task A is masked in A's log lines (and a `forDynamicTaskRun` child inherits the parent
list, `RunContextLogger.java:296`), but a later task B that never resolves it does not mask it.
**egret divergence (stricter, accepted):** the egret masking set is applied to every line of the
Surface that carries it, but a value a render resolves at run time is not persisted anywhere — the
only durable feed is `executions.secret_inputs` / `secret_outputs`. D2's "execution-wide" therefore
means "every line of the attempt/render that resolved it"; execution-wide persistence exists only
for sealed inputs (D3). Each lifted surface must mask its own attempt's lines AND its own failure
message with the values it resolved.

### 6.2 M2 — inputs posture: Kestra STORES the plaintext (non-SECRET types)

`FlowInputOutput.resolveInputValue` (`runners/FlowInputOutput.java:305-408`) renders a `defaults`
expression with the execution run context (`resolveDefaultValue`, `:372-381`, `:413-427`; its own
comment names `secret()`). The rendered value goes through `parseType` (`:538-552`):
`SELECT/STRING/EMAIL -> current.toString()` is stored as plaintext in the execution's inputs; only
`type: SECRET` is encrypted (`EncryptionService.encrypt` -> `EncryptedString`, `:542-550`). The
consumer masks it only in the logger of the run context that rendered it (`RunContextFactory.of(flow,
execution, ...)`, `runners/RunContextFactory.java:113-139`). SECRET inputs are decrypted again for
expressions (`:322`).

**D3 DECISION: SEAL, through `secret_inputs::split`** — a deliberate, stricter divergence from
Kestra. Reasons: (a) Kestra's own behaviour is "plaintext in the stored execution", which in egret
would put the secret in `workflow.executions.inputs` and in the API's execution body in clear — the
exact class `secret_inputs` exists to prevent; (b) sealing is invisible to rendering (`Surface::load`
reveals and redacts, `worker/render.rs:230`); (c) the alternative "redact-only" fixes the logs but not
the stored row. **Two facts Task 3 must carry:**
1. egret never renders a templated default at start: `inputs_check` stores nothing for it
   (`inputs_check.rs:359-371`, "not validated, rendered only for a dependent to see") and
   `worker::render::with_declared_defaults` (`render.rs:1036-1082`) inserts the **raw template source**,
   which today resolves only because `render_str` re-renders its own output (multi-pass). The 1A
   ruling's `render_with_secrets` is **one pass** (`runners/secrets.rs` header), so a `{{ inputs.x }}`
   whose default is `{{ secret('X') }}` would NOT resolve through it. The templated default must be
   rendered with the resolver **at start**, and the value stored (sealed) — as Kestra does.
2. `secret_inputs::split` selects ids by declared `type: SECRET` only (`secret_inputs.rs:68-80`, via
   `secret_ids`). Sealing a STRING input that resolved a secret needs `split` (or a sibling) to take
   the set of ids to seal.

### 6.3 M3 — trigger timing

Schedule: the trigger run context is `runContextFactory.of(flow, trigger)` in the scheduler
(`scheduler/.../TriggerScheduler.java:179,203`), with its own `RunContextLogger`
(`RunContextFactory.java:176-193`; consumer wired by `RunVariables.build` `:567`). `when` renders per
candidate tick at trigger evaluation (`plugin/core/trigger/Schedule.java:372-381, 548-559`); `inputs`
and `labels` render when the execution is built, still at evaluation, before the execution exists
(`plugin/core/trigger/SchedulableExecutionFactory.java:60-87, 91-108`). Webhook: at **request time**,
`WebhookService.newExecution` (`services/WebhookService.java:147-178`) renders labels
(`LabelService.fromTrigger`), `conditionService.isValid`, then `runContext.render(trigger.getInputs())`;
the `key` is rendered per request in `ExecutionController.findWebhook` (`webserver/.../
ExecutionController.java:889-890`). Values are masked in the **trigger's own** logger only (the
created execution has its own logger and does not inherit the list). Trigger inputs then flow through
`readInputs`, so a SECRET-typed target input encrypts and a STRING one stores plaintext (as M2).

### 6.4 M4 — runIf

`runIf` renders to a string and `TruthUtils.isTruthy` (`utils/TruthUtils.java:5-14`: false for
`false`, `0`, `-0`, empty) — a boolean; the value is never surfaced
(`executor/.../ExecutorService.java:1298,1377`; `executor/.../handler/ExecutionEventMessageHandler.java:269`;
`worker/.../WorkerTaskProcessor.java:138`). The only path to text is the error message, logged on the
task's logger (`ExecutionEventMessageHandler.java:317-318`, `WorkerTaskProcessor.java:153`), which the
logger masks. Executor-side runIf uses `RunContextFactory.of(flow, task, execution, taskRun)` with a
logger, so the consumer is wired.

### 6.5 What egret has at each surface TODAY (seams; worktree `05f737230`)

- **Resolver:** `runners::secrets::render_with_secrets(props: &Map<String, Value>, cx: &RenderContext,
  workspace: Uuid, source: &dyn SecretSource) -> Result<(Map<String, Value>, Resolved), String>`
  (`secrets.rs:141`, async, one pass, error already masked). **`Resolved.values: Vec<String>` is a
  public field** (`:123-125`) — there is no accessor method. Redaction feed:
  `RenderContext::redact(&mut self, &[String])` / `redactions()` (`runners/env.rs:413-420`).
- **Log:** `worker::handlers::log::Log::run` (`worker/handlers/log.rs:91`, `render_str(text,
  &task.render)`); `Log` holds no `SecretSource` (kestra/http handlers hold `rt.secrets`,
  `kestra.rs:204`, `http.rs:200`), and its masked log is built from `task.render.redactions()`
  (`worker/claim.rs:272`).
- **runIf:** `worker::render::render_run_if` (`worker/render.rs:1763`), **synchronous**
  `crate::render_str`, called from `claim.rs:5605` inside a settle pass — a synchronous render cannot
  await a secret fetch, so the surface needs a pre-fetch (the collect/fetch/resolve split) before the
  pass. Failure of a runIf is silent (no message) except a read limit (`failed_on_a_limit`).
- **Inputs at start:** `worker::claim::checked_inputs` / `checked_inputs_for` / `input_renderer`
  (`claim.rs:4279-4330`) -> `inputs_check::resolve` (`InputRenderer` trait, `inputs_check.rs:137`):
  renders `dependsOn.condition`, SELECT/MULTISELECT `expression`, and a templated default only for a
  depended-on input. Synchronous, no secret source; `start_execution_in_on` (`claim.rs:4347`). Stored
  form: `secret_inputs::split` at the start (`secret_inputs.rs:80`).
- **Triggers:** egret has NO Schedule `when` (refused: `ScheduleError::UnsupportedWhenCondition`,
  `trigger/schedule.rs:1277-1300`, roadmap #11) and refuses a templated Schedule `inputs` value at fire
  (`scheduler/fire.rs:595-606` `refuse_templated`; KI-122 save warning). Real render seams today:
  Webhook `inputs` (`trigger::webhook::render_inputs`, `webhook.rs:1007`, called at
  `webhook_routes.rs:645`; sync, no source) and Webhook `key` (`render_key`, `webhook.rs:540`),
  Flow-trigger `when`/`inputs` (`flow_trigger/process.rs:955 render_when`, `start_fire` `:996`).
  There is no Webhook `conditions` render; trigger `labels` are a model field (`models/trigger.rs:51`)
  with no render seam found. **The "trigger" surface of 10a is narrower than the spec's
  "Schedule/Webhook condition and labels": see §6.7.**

### 6.6 Lift map (grep authority; code + message + fixture YAML)

Refusal CODE `secret_outside_plugin_task` (`plugins/mod.rs:177`), emitted by `spec/validate.rs:614-621`
(`Walk::outside`, which refuses a `secret()` in EVERY string that is not a `kestra.*`/http task's
properties — so each lift must be an allow-list of paths, not a removal; `Return.format`, outputs
`value:`, Set/Put `value`, `key`, script commands must stay refused).
Refusal MESSAGE: `functions::SECRET_REFUSAL` (`runners/functions.rs:323-326`, text "resolves only on
`kestra.*` plugin tasks ... no other render (inputs, runIf, Log, triggers) resolves secrets" — becomes
false and must be reworded; every assertion below on that substring moves with it). Wiring:
`functions.rs:388` (`("secret", Refusal::Confined(SECRET_REFUSAL))`), re-export `runners/mod.rs:95`.

Code/message sites (src): `src/plugins/mod.rs:174-193` (doc + `CODE_SECRET_OUTSIDE_PLUGIN_TASK`,
`CODE_SECRET_IN_KV_VALUE`), `src/spec/validate.rs:541-707` (walker + header) and its tests `:5881-6031`
(`a_secret_in_a_set_or_put_value...`, `a_secret_elsewhere_on_a_set_task...`,
`a_secret_in_an_http_tasks_properties_is_allowed...` — **asserts `runIf` and a `Log` beside an http
task are refused: `:6013-6015`, `:6026-6027`**), `:6816-6830` (outputs `value:` — stays refused),
`src/runners/secrets.rs:1-10` (header prose) and tests `:997, :1026, :1085, :1102`
(`resolves only on \`kestra.*\``), `src/runners/functions.rs:3795` (constant-keyed),
`src/runners/nested.rs:55-67,925` (nested `render()` stays refused — keep), `src/outputs.rs:516-535`
(output `value:` — keep), `src/worker/handlers/storage/items.rs:42,2059-2095` (FilterItems expression
— keep), `src/storage/prefetch.rs:73` (keep), `src/eval.rs:13-22,403-422` (eval seal — keep),
`src/secret_inputs.rs:37` (prose).

Tests (tests/): `tests/plugins_gate.rs:1027-1110` (**`secret_outside_a_plugin_tasks_properties_is_refused_by_name`
— its cases are literally "a plugin task's runIf" (`tasks.q.runIf`), "an input's defaults"
(`inputs.i.defaults`), "a Log under a flowable's errors" (`tasks.s.errors.e.message`), "a trigger"
(`triggers.w.key`), plus a script `commands` case; asserts the "resolves only on" message**),
`tests/plugins_gate.rs:1133-1135,1158-1170` (argument/detector tests — keep), `:813-1000, 1192`
(WF_USE_SECRETS gate — keep, D5), `tests/publish.rs:917-960` (KV value; negative assertion on the
message substring), `tests/executions_routes.rs:4090-4302` (eval seal — keep). Not touching a refused
surface: `tests/http_*.rs`, `tests/plugin_host_live.rs`, `tests/secret_inputs.rs` (names only).

Fixture YAML (oracle-exec): `oracle-exec/corpus/valids/secrets.yaml` (Return `format:` x4 + a Log
`message:`; stays refused after 10a because of `Return.format`), and
`oracle-exec/corpus/valids/webhook-secret-key.yaml` (Webhook `key:`). Both are named in
`oracle-exec/driver.rs:743-751` (the "245" census note) and sit in `VALIDATE_REFUSED`
(`driver.rs:957-960`, "22"); also `oracle-exec/w5b-plugins.json`, `w9a-storage.json` (plugin-task
`secret()` only; untouched). **CORPUS-COUPLING WARNING:** the spec's "banks 0 corpus flows" holds only
if the trigger lift leaves Webhook `key` refused. A lift that allows `secret()` anywhere under
`triggers:` moves `valids/webhook-secret-key` out of `VALIDATE_REFUSED` and trips the census.
Decision: keep `triggers.*.key` refused in 10a (see §6.7).

`source_uses_secret` (`runners/secrets.rs:333-348`) walks **every** YAML string leaf and mapping key
of the whole source (aliases resolved, double-quoted escapes decoded), so it covers Log, runIf,
inputs and triggers: D5 CONFIRMED, no surface escapes it. It gates create/update/publish
(`routes.rs:697,824,1159`); a start is not separately gated (the source was gated when saved).

### 6.7 Divergences and concerns recorded for later tasks

1. Masking is per-logger in Kestra; egret masks per resolving render/attempt (+ sealed inputs).
2. Inputs: seal (stricter than Kestra's plaintext-for-non-SECRET); needs render-at-start for templated
   defaults and a `split` that takes an id set (§6.2).
3. Triggers: only Webhook `inputs` and Flow-trigger `when`/`inputs` have a render seam; Schedule
   `when` (#11) and templated Schedule `inputs` are refused outright and are NOT 10a. Webhook `key`
   stays refused (corpus coupling, and it is a credential compared per request).
4. runIf is synchronous in a settle pass: Task 2 needs a pre-fetch before the pass.

## 7. Scope

**In (as built):** `secret()` resolves at four surfaces, each masked and walker-allow-listed:
Log `message:`, task `runIf:` (verdict stored, never the text), flow input `defaults:` (sealed like a
`SECRET` input), and Webhook `inputs:` + Flow-trigger `inputs:`/`when:` (inputs sealed, `when` a
verdict). Every handed-out value is masked in logs; the inputs seal posture per M2; tests; docs.

**Out / still refused:** KV `Set`/`Put` value and key, task and flow output `value:`,
`Return.format`, script `commands`, Webhook `key:`, Schedule templated `inputs:`/`when:`, `dependsOn`
dependency `when:`, Pause `onResume`, and the eval endpoint seal. Also out: `secret()` in
script/container **env** (-> 10b), the Kubernetes runner (-> #17), any new secret *source*. Banks are
unchanged (census tuple, `LIVE` 299, `VALIDATE_REFUSED` 21): `valids/webhook-secret-key` (Webhook
`key`) and `valids/secrets` (`Return.format`) stay refused.

**As-built notes.** Reads go through the per-attempt `Prefetch` (a third kind, `secret`), because
runIf/inputs/trigger renders are synchronous inside a transaction. A seal set is *derived* from the
declaration (every `SECRET` input, every input whose default calls `secret()`, every key a trigger
supplies from a `secret()` template, declared or not), so replay and Subflow re-seal on every path.
`workflow:use_secrets` is still required to save or publish a flow that calls `secret()` at any surface.

**Known limitations (follow-ups, both fail safe):**

- A `secret()` in a flow **input default** resolves only on a top-level start (scheduler, API,
  Schedule-trigger). On a **Subflow child**, a **replay with Replace inputs**, a **Pause `onResume`**
  and a **Flow-trigger fire**, the default stays raw and the run fails at render with the secret
  refusal (no leak). Follow-up: extend those paths.
- During a **secret-store outage**, a Flow-trigger `when` that calls `secret()` is treated as
  unrenderable and the event is refused/skipped (data loss, not a leak). Follow-up: align with how
  `runIf` surfaces the outage.

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
