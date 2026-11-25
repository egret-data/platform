# Package 10b — `secret()` in script-task env — Design

**Status:** approved design, pre-plan
**Date:** 2026-11-25
**Roadmap:** egret-workflow Kestra parity, package #10 ("secret() everywhere"), part **10b**. Part 10a (`secret()` at in-process render surfaces: Log, runIf, inputs, triggers) merged as PR #913 on 2026-11-25.

## 1. Goal

Let `secret('NAME')` resolve inside a script task's `env:` map, so a secret can be
handed to user code running in a container/pod — injected **securely where the
runtime allows it** — while every other script-task field keeps refusing
`secret()`. First time script stdout/stderr is masked for secret values.

In scope (the only field lifted): `tasks.*.env` **values** on script tasks
(`io.kestra.plugin.scripts.{python,shell}.{Script,Commands}`, `egret.container`,
`egret.script.python`).

Explicitly **out of scope** (stay refused, both at save and run):
`commands:`, `script:`, `beforeCommands:`, `interpreter:`, `inputFiles:`,
`outputFiles:`, `namespaceFiles:`, image/container fields. Users reference a
secret inside their script through the environment variable (`$NAME`), never by
inlining `secret()` into a command. Package 10b is **env-only**.

## 2. Why env-only + secure-where-possible

Measured facts (Kestra @ 96893b589; egret worktree):

- **Kestra has no secret channel.** `{{ secret('X') }}` in `env:` is ordinary
  Pebble rendering into a plain env var; the runner passes it as plaintext to the
  container (`docker --env KEY=VALUE`) or the pod spec. Secrecy rests **only** on
  log masking (one unified `RunContextLogger.useSecrets` set, raw + base64,
  replaced in the log appender). `env` is not written to disk; `inputFiles` is.
- **egret deliberately refuses it today.** `secret()` is `Refusal::Confined` in
  the confined render engine; the refusal message states the rationale: on a
  script task the rendered env/commands are fields of the pod spec, readable by
  anyone with `get pods` in the task namespace.

The secure mechanism that removes that exposure — a k8s Secret object referenced
by `secretKeyRef` — **only works for env vars**. A secret inlined into
`commands:`/`script:` is substituted into argv and lands in the pod spec's
`command`/`args` and in execution history; `inputFiles` lands on disk. Neither
can be secured. Therefore the secure posture **forces env-only**: lift `env:`,
keep refusing the rest, and tell users to reference `$NAME`.

**Decisions (user-approved):**

- **D1 — Posture:** secure-where-possible, stricter than Kestra on k8s.
- **D2 — Scope:** `env:` values only.
- **D3 — k8s:** inject secret-bearing env entries via a per-attempt k8s **Secret
  object** + `secretKeyRef`; the value is absent from the pod spec. The Secret
  carries an **ownerReference to the Pod** so k8s garbage-collects it when the pod
  is deleted (no manual delete, no delete RBAC needed).
- **D4 — Docker:** no secure equivalent exists; pass secret-bearing env through
  **`--env-file`** (mode 0600, `unlink`ed immediately after `docker create`) so
  the value is not visible in the host `ps`. The value remains visible in
  `docker inspect` — a documented residual exposure (parity with Kestra on
  docker).
- **D5 — Masking:** wire `task.log.masked(&resolved.values)` into the script
  handler for the first time; mask stdout/stderr and any echoed command line.
- **D6 — RBAC:** the engine's k8s service account must have `create` on Secrets
  in task namespaces. Accepted operational cost of D3.

## 3. Architecture

One seam in the script handler, plus one new helper per runner.

### 3.1 Resolution (shared, `worker/handlers/script.rs`)

Today `Script::render` builds the env with `render_values(type_id, "env", &p.env, &cx)`
(container.rs → `render_str`, the **confined** engine that refuses `secret()`).

Replace the env render with the 10a secret-aware path
`runners::secrets::render_with_secrets` (the same call `log.rs`/`http.rs` use). It:

- resolves `secret()` using a working secret source,
- returns the rendered map **and** `Resolved.values` — the list of secret values
  seen, for masking.

Env rendering happens **async inside the handler run** (not inside a settle-pass
Postgres txn like 10a's runIf/inputs). So resolution can simply `await` the
secret fetch; a fetch failure fails the attempt cleanly. **No Prefetch machinery
is needed** (that was only for the sync-in-txn surfaces of 10a).

Per-entry classification: an env entry is **secret-bearing** if its template used
`secret()`. `render_with_secrets` is applied per value (or the resolved-values
set is intersected with each rendered value) so the handler knows, per key,
whether to route it through the secure channel or inline it as before. Non-secret
env keeps the existing inline path unchanged.

The handler passes to the runner, in `TaskCommands`:
- `env`: the non-secret entries (inline, as today), and
- a new `secret_env: BTreeMap<String,String>`: the secret-bearing entries
  (key → resolved value), for the runner to inject securely.

`resolved.values` is handed to `task.log.masked(...)` (D5).

### 3.2 k8s runner (`worker/k8s/`)

For each attempt where `secret_env` is non-empty:

1. **Create a Secret** `egret-task-<exec8>-<task>-<attempt>` (name derived,
   unique per attempt) in the pod's namespace, `data` = base64 of each
   `secret_env` value, with `ownerReferences: [<the Pod>]` (`controller: false`,
   `blockOwnerDeletion: false`).
2. **Pod spec** (`pod_spec.rs`): for each `secret_env` key emit
   ```
   EnvVar { name: KEY, value: None,
            value_from: Some(EnvVarSource { secret_key_ref:
              Some(SecretKeySelector { name: <secret>, key: KEY, optional: Some(false) }) }) }
   ```
   instead of an inline `value`. Non-secret env stays inline.
3. **Lifecycle:** create the Secret, then create the Pod with the ownerReference
   pointing at it — ordering detail resolved in the plan (create Pod first to get
   its UID, then create Secret with that UID as owner; or create Secret, create
   Pod, then patch the Secret's ownerReference to the Pod UID). Either way, when
   the pod is deleted, GC removes the Secret. No explicit delete path.
4. **RBAC failure** (no create permission) → fail the attempt with a clear,
   actionable error naming the namespace and the missing verb.

### 3.3 docker runner (`worker/commands.rs`)

For each attempt where `secret_env` is non-empty:

1. Write a temp **env-file** (`KEY=VALUE\n` lines) to a private path, mode `0600`.
2. Add `--env-file <path>` to `docker create` (secret-bearing entries only;
   non-secret env continues as `--env KEY=VALUE`).
3. `unlink` the env-file immediately after `docker create` returns (the daemon
   has already read it).
4. The value is still retrievable via `docker inspect` — documented residual
   exposure.

### 3.4 Save-time validation (`spec/validate.rs`)

Add one allow-list arm in the `secret_outside_plugin_task` walker: for a **script
task**, `env` **values** resolve `secret()` (treat like the Log `message` arm).
Every other script-task field continues to hit `Walk::outside` →
`CODE_SECRET_OUTSIDE_PLUGIN_TASK`.

Update `SECRET_REFUSAL` (functions.rs): the message currently says `secret()` is
refused on script tasks outright. Change it to: `secret()` resolves in a script
task's `env:`; to use a secret in a command, put it in `env:` and reference
`$NAME` in your script. Keep naming the fields that still refuse.

## 4. Error handling & edge cases

- **Secret-store outage during env render:** the handler's async resolve fails →
  **fail the attempt** (fail-safe; no partial/plaintext injection, no leak).
  Simpler than 10a because there is no open txn to roll back.
- **Replay / retry:** re-resolve per attempt; the k8s Secret name is per-attempt;
  GC follows the pod. No cross-attempt reuse of a stale Secret.
- **env key collision with framework vars** (`WORKING_DIR`, `OUTPUT_DIR`, …):
  unchanged behaviour — author env extends after the defaults; a secret-bearing
  author key overrides as an author key always did.
- **Mixed env** (some secret, some not): non-secret entries inline, secret
  entries via the secure channel, in the same pod/container.
- **docker + no secret_env / k8s + no secret_env:** unchanged from today
  (no Secret object, no env-file).
- **Only two runners exist** (docker, k8s); both are covered.

## 5. Testing

- **Task 0 (measure-first, like 10a):** check the test corpus / census for any
  flow that places `secret()` in a script-task `env` and currently sits in
  `VALIDATE_REFUSED`. Lifting flips it to valid and would move the census/oracle
  tuples. Decide, as 10a did (keep a narrow refusal to hold the census stable, or
  accept a documented ratchet change) **before** writing code.
- **`tests/env_secret.rs`** (integration): a script task with
  `env: {TOK: "{{ secret('X') }}"}` resolves; the script can read `$TOK`; the
  value is masked in captured logs even when the script echoes it.
- **k8s unit** (`pod_spec.rs`): a secret-bearing env key emits `value_from`
  secretKeyRef (not inline `value`); a non-secret key stays inline.
- **docker unit** (`commands.rs`): secret-bearing env goes through `--env-file`;
  the file is `unlink`ed after create; non-secret env stays `--env`.
- **`tests/plugins_gate.rs`:** script `env:` with `secret()` now **saves**;
  `secret()` in `commands:`/`script:`/`inputFiles:` still **refused at save**;
  still needs `workflows.use_secrets` to save.
- **Save-validation flip:** the previously-refused script-env case now validates;
  the still-refused cases keep their finding.
- **Scoped test runs MUST include** `--test routes --test execution_timestamps`
  (CI parity rule).
- **Live verify on kind** (live check beats review): run a script task with
  `secret()` in `env`; confirm `kubectl get pod <p> -o yaml` shows a
  `secretKeyRef` (not the value), the Secret exists and is owned by the pod, the
  pod/Secret are GC'd together, and the log is masked.

## 6. Docs

- `platform/workflows/flows/expressions.md` — "Where secret() works": add script
  `env:`; state the `$NAME` pattern; `:::caution` the `docker inspect` residual
  exposure.
- Deployment docs — the engine k8s service account needs `create` on Secrets in
  task namespaces (RBAC snippet).
- Known-issues — any residual limitation (docker `inspect` exposure) if not fully
  covered by the caution.

## 7. Global constraints

- Chat Vietnamese; code/commits/docs English.
- Conventional Commits; `Closes #N` only in the PR body.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`;
  PR body ends with the Claude Code generation line.
- Never commit secrets or scratch files. Stage by explicit path; never
  `git add -A`; never stage `target*/`.
- ONE heavy Rust build at a time; builds run foreground in subagents.
- Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- Scoped `egret-workflow` test runs include `--test routes --test execution_timestamps`.
