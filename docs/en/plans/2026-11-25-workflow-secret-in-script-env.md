# secret() in script-task env (package 10b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `secret('NAME')` resolve inside a script task's `env:` map, inject it securely where the runtime allows (k8s Secret + secretKeyRef; docker `--env-file`), mask the values in script stdout/stderr, and keep every other script-task field refusing `secret()`.

**Architecture:** One async resolution seam in the Script handler (`render()` is sync and refuses `secret()` in `env`, so author-env rendering moves out of `render()` into the async `run()`, using the 10a `render_with_secrets` path). Secret-bearing env entries are split into a new `TaskCommands::secret_env` and injected by each runner through a secure channel; a masked `task.log` handle (10a `RunLog::masked`) is threaded into both runners so script output is redacted.

**Tech Stack:** Rust, `crates/egret-workflow`; axum + sqlx/Postgres; docker CLI runner; k8s runner (`k8s-openapi` `EnvVar`/`Secret`). Reuses 10a machinery: `runners::secrets::{render_with_secrets, current, text_uses_secret, Resolved}`, `logs::RunLog::masked`.

## Global Constraints

- Chat Vietnamese; code/commits/docs English.
- Conventional Commits; `Closes #N` only in the PR body (never in commit messages).
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Never commit secrets or scratch files. Stage by explicit path; never `git add -A`; never stage `target*/`.
- ONE heavy Rust build at a time; builds run FOREGROUND in subagents.
- Scoped `egret-workflow` test runs MUST include `--test routes --test execution_timestamps` (CI parity) — e.g. `cargo test -p egret-workflow --test env_secret --test plugins_gate --test routes --test execution_timestamps`.
- Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- secret() stays **env-only** on script tasks: `commands:`, `script:`, `beforeCommands:`, `interpreter:`, `inputFiles:`, `outputFiles:`, `namespaceFiles:`, image fields keep refusing.
- DB for local runs: set both `DATABASE_URL` and `EGRET_DATABASE__URL` to the worktree's test Postgres.

## File Structure

- `crates/egret-workflow/src/worker/commands.rs` — `TaskCommands` gains `secret_env` + `secret_masks`; manual `Debug` redacts them; `stream_line`/`stream_replayed_line` take `log: &RunLog` instead of `task`; `DockerRunner` builds the masked log + the `--env-file` channel.
- `crates/egret-workflow/src/worker/handlers/script.rs` — `render()` stops rendering author `env` and returns the render context; `run()` gains the async `resolve_env` seam producing `env`/`secret_env`/`secret_masks`.
- `crates/egret-workflow/src/worker/k8s/pod_spec.rs` — secret-bearing env emits `value_from` secretKeyRef.
- `crates/egret-workflow/src/worker/k8s/runner.rs` — creates the per-attempt Secret with an ownerReference to the Pod; builds the masked log.
- `crates/egret-workflow/src/spec/validate.rs` — script-task `env` allow-arm + `script` membership set.
- `crates/egret-workflow/src/runners/functions.rs` — `SECRET_REFUSAL` message text.
- Tests: `crates/egret-workflow/tests/env_secret.rs` (new); additions to `tests/plugins_gate.rs`; unit tests in `pod_spec.rs` and `commands.rs`.
- Docs (separate repo `egret-data/docs`): `platform/workflows/flows/expressions.md`, deployment RBAC doc, `platform/.../known-issues.md`.

---

### Task 0: Census / Banks measurement (measure-first, no production code)

**Files:**
- Read only: `crates/egret-workflow/tests/executor.rs`, `crates/egret-workflow/oracle-exec/`, any corpus/fixture under `crates/egret-workflow/tests/` and `oracle-exec/` that carries flows.
- Produce: a short written finding in the task report.

**Why:** 10a kept the census stable by leaving one surface (`triggers.*.key`) refused. 10b lifts script-task `env`. If any corpus/oracle flow currently places `secret()` in a script-task `env` and sits in a "VALIDATE_REFUSED" (or equivalent refused) bucket, lifting flips it to valid and moves the census/oracle tuples. We must know this **before** writing code and decide: keep the census stable (no corpus flow depends on script-env refusal) or record a deliberate ratchet change.

- [ ] **Step 1: Grep the corpus for script-env secret usage**

Run (from `crates/egret-workflow`):
```bash
grep -rn "secret(" tests/ oracle-exec/ | grep -iE "env|script|shell|container" 
grep -rln "secret(" tests/ oracle-exec/
```
Also inspect how refused flows are catalogued (search for the refusal bucket names, e.g.):
```bash
grep -rn "VALIDATE_REFUSED\|PublishedButRefused\|census\|REFUSED" tests/ oracle-exec/ src/ | head -40
```

- [ ] **Step 2: Decide and record**

In the task report state, with file:line evidence, one of:
- **(A) Census unchanged:** no corpus/oracle flow puts `secret()` in a script-task `env`; lifting changes no census/oracle tuple. (Expected, mirrors 10a's analysis.) Proceed.
- **(B) Census moves:** list each flow that would flip, and the decision (accept the documented ratchet, or add a narrow guard). If (B), surface to the controller before Task 1 — this is a plan-affecting fact.

- [ ] **Step 3: Commit the finding** (docs/ledger only — no source change)

No source files change in Task 0. Record the finding in the SDD ledger / task report; nothing to commit in-tree unless a corpus note file is warranted.

---

### Task 1: `TaskCommands` secret fields + masked-log plumbing (no behavior change)

**Files:**
- Modify: `crates/egret-workflow/src/worker/commands.rs` (`TaskCommands` struct ~212, manual `Debug` ~358, `stream_line` ~719, `stream_replayed_line` ~748, `DockerRunner::run` streaming loop, fake runners in `#[cfg(test)]`).
- Modify: `crates/egret-workflow/src/worker/k8s/runner.rs` (its streaming loop ~355-380).

**Interfaces:**
- Produces:
  - `TaskCommands { … , pub secret_env: BTreeMap<String,String>, pub secret_masks: Vec<String> }` — `secret_env` = secret-bearing env entries (key→resolved value) for the secure channel; `secret_masks` = every secret value to redact in logs (from `Resolved.values`). Both default empty (`#[derive(Default)]` already present).
  - `stream_line(log: &RunLog, observer, result, state, max_vars_bytes, line, is_err, host_protocol)` and `stream_replayed_line(log: &RunLog, …, replayed)` — first param is now the log handle, not `&TaskInput` (they only ever used `task.log`).

- [ ] **Step 1: Add the fields + redacting Debug**

In `TaskCommands` (commands.rs ~212) add after `env`:
```rust
    /// Secret-bearing env entries (key -> resolved value), injected through a
    /// SECURE channel by the runner (k8s Secret+secretKeyRef; docker --env-file),
    /// never inline in the pod spec / docker CLI. Package 10b.
    pub secret_env: BTreeMap<String, String>,
    /// Every secret value to redact from this task's stdout/stderr (from
    /// `runners::secrets::Resolved.values`). Package 10b.
    pub secret_masks: Vec<String>,
```
Update the manual `Debug` (commands.rs ~358): print `secret_env` as key names only, and never print `secret_masks` values:
```rust
            .field("env", &self.env)
            .field("secret_env", &self.secret_env.keys().collect::<Vec<_>>())
            .field("secret_masks", &format_args!("<{} values>", self.secret_masks.len()))
```

- [ ] **Step 2: Refactor the stream fns to take `log: &RunLog`**

In `stream_line` / `stream_replayed_line` (commands.rs ~719, ~748) replace the `task: &TaskInput` parameter with `log: &crate::logs::RunLog`, and replace every `task.log` inside with `log` (`log.line(...)`, `log.stored_form(...)`). Update their doc comments' `task.log` references to `log`.

- [ ] **Step 3: Update both runners to build a masked log and pass it**

In `DockerRunner::run` and `KubernetesRunner::run`, immediately before the streaming loop add:
```rust
let log = task.log.masked(&cmds.secret_masks); // empty masks == identity
```
and pass `&log` to every `stream_line` / `stream_replayed_line` call in that runner. Any direct `task.log.line(...)` inside the streaming path (e.g. the k8s stderr-not-separable WARN at runner.rs ~369) should also write through `log` so it is masked. (`RunLog::masked(&[])` returns a clone with the same mask set, so this is a no-op when there are no secrets — verified at run_log.rs `masked`.)

- [ ] **Step 4: Fix call sites in test fakes**

Update the `#[cfg(test)]` fake runners / helpers in commands.rs that call `stream_line`/`stream_replayed_line` to pass `&task.log` (or a masked handle). The existing masking test at commands.rs ~4002 (`task.log = task.log.masked(&["pa55"…])`) should keep passing.

- [ ] **Step 5: Build + test (foreground)**

```bash
cargo test -p egret-workflow --test routes --test execution_timestamps
cargo test -p egret-workflow --lib worker::commands
```
Expected: PASS. This task is pure plumbing — no flow can populate `secret_env`/`secret_masks` yet, so behavior is unchanged.

- [ ] **Step 6: Commit**
```bash
git add crates/egret-workflow/src/worker/commands.rs crates/egret-workflow/src/worker/k8s/runner.rs
git commit -m "feat(workflow): TaskCommands secret_env/secret_masks + masked-log plumbing"
```

---

### Task 2: Handler resolution seam (`render()` sync → async resolve in `run()`) + masking

**Files:**
- Modify: `crates/egret-workflow/src/worker/handlers/script.rs` (`render()` ~247, the `Rendered` struct, `run()` ~621).
- Test: `crates/egret-workflow/tests/env_secret.rs` (new).

**Interfaces:**
- Consumes: `TaskCommands::{secret_env, secret_masks}` (Task 1); `runners::secrets::{render_with_secrets, current, text_uses_secret, Resolved}` (secrets.rs: `render_with_secrets(props: &Map<String,Value>, cx: &RenderContext, workspace: Uuid, source: &dyn SecretSource) -> Result<(Map<String,Value>, Resolved), String>`; `current() -> Option<Arc<dyn SecretSource>>`; `text_uses_secret(&str) -> bool`; `Resolved.values: Vec<String>`).
- Produces: `cmds.env` (non-secret author env + python defaults, inline as today), `cmds.secret_env`, `cmds.secret_masks`.

**Design note:** `render()` is **synchronous** and renders author `env` via `render_values`→`render_str` (the confined engine that refuses `secret()`). So author-env rendering must leave `render()` and happen in the async `run()`. `render()` keeps building the render context `cx` (with `WORKING_DIR_VAR`/`OUTPUT_DIR_VAR`) and all non-env props; it stops populating author env into `cmds.env` (python `ENV_DEFAULTS` stay). `run()` then resolves `parsed.env` against the returned `cx`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/egret-workflow/tests/env_secret.rs`. Pattern it on the existing 10a secret tests (e.g. `tests/log_secret.rs`) for installing a fake `SecretSource` and a fake runner that echoes env. Assert:
```rust
// A script task with env: {TOK: "{{ secret('API') }}", PLAIN: "hi"}:
//  - resolves TOK to the secret value (reaches the runner as secret_env),
//  - PLAIN stays in cmds.env (inline),
//  - the secret value is MASKED in captured stdout when the script echoes $TOK,
//  - secret_masks contains the resolved value.
```
(Use the same fake-SecretSource + capture-log harness the 10a tests use; mirror their setup helpers. If a reusable test harness exists under `tests/common` or inside log_secret.rs, reuse it.)

- [ ] **Step 2: Run it — expect FAIL** (today env `secret()` is refused by `render_values`)

```bash
cargo test -p egret-workflow --test env_secret --test routes --test execution_timestamps
```
Expected: FAIL (render refuses `secret()` / secret_env empty).

- [ ] **Step 3: `render()` — stop rendering author env; return `cx`**

In `script.rs render()` (~271) delete `let author_env = render_values(type_id, "env", &p.env, &cx)?;` and the later `env.extend(author_env);` (~334). Keep building `env` from `ENV_DEFAULTS` for python. Add `cx` to the `Rendered` struct so `run()` can reuse the exact context:
```rust
struct Rendered { cmds: TaskCommands, ns_inputs: …, namespace_files: …, cx: RenderContext }
```
and set `cx` in the returned value.

- [ ] **Step 4: `run()` — async resolve_env after render(), before the gate**

In `run()` (~641, right after the `render()` match that binds `cmds`, `cx`), add:
```rust
// Package 10b: resolve the author env with secret() support, splitting the
// secret-bearing entries into the secure channel and collecting the mask set.
match resolve_env(task, &parsed.env, &cx).await {
    Ok((plain, secret_env, masks)) => {
        cmds.env.extend(plain);            // non-secret entries inline, as before
        cmds.secret_env = secret_env;
        cmds.secret_masks = masks;
    }
    Err(why) => return Ok(Outcome::failed_with(why)),
}
```
Add the helper (same file):
```rust
async fn resolve_env(
    task: &TaskInput,
    author_env: &std::collections::BTreeMap<String, String>,
    cx: &RenderContext,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>, Vec<String>), String> {
    if author_env.is_empty() {
        return Ok((BTreeMap::new(), BTreeMap::new(), Vec::new()));
    }
    match crate::runners::secrets::current() {
        // use_secrets on: render with secret() support.
        Some(source) => {
            let mut props = serde_json::Map::new();
            for (k, v) in author_env {
                props.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            let (rendered, resolved) = crate::runners::secrets::render_with_secrets(
                &props, cx, task.workspace_id, source.as_ref(),
            ).await.map_err(|e| format!("{}: `env` did not render: {e}", task.type_id))?;
            let mut plain = BTreeMap::new();
            let mut secret_env = BTreeMap::new();
            for (k, raw) in author_env {
                let value = rendered.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
                if crate::runners::secrets::text_uses_secret(raw) {
                    secret_env.insert(k.clone(), value);
                } else {
                    plain.insert(k.clone(), value);
                }
            }
            Ok((plain, secret_env, resolved.values))
        }
        // use_secrets off: confined render (today's path). secret() errors here,
        // but it is refused at save, so a stored flow never reaches this arm with one.
        None => {
            let plain = render_values(&task.type_id, "env", author_env, cx)
                .map_err(|e| e.to_string())?;
            Ok((plain, BTreeMap::new(), Vec::new()))
        }
    }
}
```
(Adjust `render_values`' error type to the handler's `String` convention as the surrounding code does; match the exact signature of `render_values` in container.rs.)

- [ ] **Step 5: Run the test — expect PASS**

```bash
cargo test -p egret-workflow --test env_secret --test routes --test execution_timestamps
```
Expected: PASS (TOK resolved into secret_env, masked in stdout; PLAIN inline).

- [ ] **Step 6: Commit**
```bash
git add crates/egret-workflow/src/worker/handlers/script.rs crates/egret-workflow/tests/env_secret.rs
git commit -m "feat(workflow): resolve secret() in script env (handler seam + masking)"
```

---

### Task 3: k8s secure channel — Secret object + secretKeyRef + ownerReference GC

**Files:**
- Modify: `crates/egret-workflow/src/worker/k8s/pod_spec.rs` (env block ~317-333).
- Modify: `crates/egret-workflow/src/worker/k8s/runner.rs` (create the Secret before/with the Pod; RBAC error).
- Test: unit test in `pod_spec.rs`.

**Interfaces:**
- Consumes: `cmds.secret_env` (Task 1/2).

- [ ] **Step 1: Write the failing pod_spec unit test**

In `pod_spec.rs` `#[cfg(test)]`, add:
```rust
#[test]
fn secret_env_emits_secret_key_ref_not_inline() {
    let mut cmds = /* minimal TaskCommands */;
    cmds.env.insert("PLAIN".into(), "hi".into());
    cmds.secret_env.insert("TOK".into(), "shh".into());
    let env = build_env(/* the fn that produces Vec<EnvVar>, + the secret name */);
    let plain = env.iter().find(|e| e.name == "PLAIN").unwrap();
    assert_eq!(plain.value.as_deref(), Some("hi"));
    assert!(plain.value_from.is_none());
    let tok = env.iter().find(|e| e.name == "TOK").unwrap();
    assert!(tok.value.is_none());
    let sel = tok.value_from.as_ref().unwrap().secret_key_ref.as_ref().unwrap();
    assert_eq!(sel.key, "TOK");
    assert_eq!(sel.optional, Some(false));
    // the resolved value "shh" must NOT appear anywhere in the EnvVar list
    assert!(!format!("{env:?}").contains("shh"));
}
```

- [ ] **Step 2: Run it — expect FAIL**
```bash
cargo test -p egret-workflow --lib worker::k8s::pod_spec
```

- [ ] **Step 3: Emit secretKeyRef for secret_env keys**

In pod_spec.rs where env is built (~317-333), after the inline `cmds.env` entries add, for each `(k, _) in &cmds.secret_env`:
```rust
env.push(EnvVar {
    name: k.clone(),
    value: None,
    value_from: Some(EnvVarSource {
        secret_key_ref: Some(SecretKeySelector {
            name: secret_name.to_string(), // the per-attempt Secret's name
            key: k.clone(),
            optional: Some(false),
        }),
        ..Default::default()
    }),
});
```
Thread the per-attempt `secret_name` into the env-building fn (derive it where the pod name is derived — `egret-run-<run id hyphenless>-<attempt>` → the Secret shares that stem, e.g. same name or `<stem>-secrets`). Keep it deterministic per (run, attempt) so a re-attach computes the same name.

- [ ] **Step 4: Create the Secret with an ownerReference to the Pod**

In `KubernetesRunner::run`, when `!cmds.secret_env.is_empty()` and not re-attaching to an existing pod: create the k8s `Secret` in the pod's namespace with `string_data`/`data` from `cmds.secret_env`, then create the Pod, then set the Secret's `ownerReferences` to the Pod (`controller: Some(false)`, `block_owner_deletion: Some(false)`) — create the Pod first to obtain its `uid`, then create the Secret with that owner (or create the Secret, create the Pod, and patch the Secret's ownerReference with the Pod uid). On re-attach, the deterministic name means the Secret already exists; do not recreate. RBAC/create failure → map to the task's failure with a clear message:
```rust
return Ok(... failed_with(format!(
  "{type_id}: cannot create the Secret for secret() env in namespace {ns}: {e} \
   (the engine service account needs `create` on Secrets there)")));
```

- [ ] **Step 5: Run the unit test — expect PASS**; run the k8s suite if DB/k8s-gated tests are present (they may skip without a cluster — note skips in the report).

- [ ] **Step 6: Commit**
```bash
git add crates/egret-workflow/src/worker/k8s/pod_spec.rs crates/egret-workflow/src/worker/k8s/runner.rs
git commit -m "feat(workflow): k8s secret() env via Secret object + secretKeyRef (owner GC)"
```

---

### Task 4: docker secure-ish channel — `--env-file` + unlink

**Files:**
- Modify: `crates/egret-workflow/src/worker/commands.rs` (`DockerRunner` `docker create` env building ~3015-3024).
- Test: unit test in `commands.rs`.

**Interfaces:**
- Consumes: `cmds.secret_env`.

- [ ] **Step 1: Write the failing unit test**

In commands.rs `#[cfg(test)]`, add a test over the create-arg builder (factor the env-arg construction into a testable helper if it isn't one) asserting: non-secret env → `--env KEY=VALUE`; secret env → `--env-file <path>` (not `--env TOK=…`); the env-file content is `TOK=shh\n` with mode `0600`; the secret value does NOT appear in the `docker create` argv.

- [ ] **Step 2: Run it — expect FAIL.**

- [ ] **Step 3: Implement the env-file channel**

Where `docker create` args are built (~3015): keep non-secret `cmds.env` as `--env KEY=VALUE`. When `!cmds.secret_env.is_empty()`, write a temp file (e.g. under the task scratch dir or `std::env::temp_dir()`), mode `0600` (`std::os::unix::fs::OpenOptionsExt::mode(0o600)`), lines `KEY=VALUE\n` for each `secret_env` entry; add `--env-file <path>`; after `docker create` returns, `std::fs::remove_file(path)` (log at debug on failure, do not fail the task). Ensure the path is removed on the error path too (RAII guard or explicit removal before every early return in that block).

- [ ] **Step 4: Run the unit test — expect PASS.**

- [ ] **Step 5: Commit**
```bash
git add crates/egret-workflow/src/worker/commands.rs
git commit -m "feat(workflow): docker secret() env via --env-file (0600, unlinked after create)"
```

---

### Task 5: Save-time validation lift + refusal message + gate tests

**Files:**
- Modify: `crates/egret-workflow/src/spec/validate.rs` (`Walk` struct ~607, `is_log` block ~683, the allow-arm chain ~702, the walk constructor ~790).
- Modify: `crates/egret-workflow/src/runners/functions.rs` (`SECRET_REFUSAL` ~325).
- Test: additions to `crates/egret-workflow/tests/plugins_gate.rs`.

**Interfaces:** none produced; this gates save.

- [ ] **Step 1: Write failing gate tests** in `plugins_gate.rs`:
```rust
// (a) a script task with env: {TOK: "{{ secret('X') }}"} now SAVES (no finding).
// (b) a script task with commands: ["echo {{ secret('X') }}"] STILL refused at save.
// (c) a script task with script: "... {{ secret('X') }} ..." STILL refused.
// (d) a script task with inputFiles: {f: "{{ secret('X') }}"} STILL refused.
// (e) the lifted env case still needs `workflows.use_secrets` to save.
```

- [ ] **Step 2: Run — expect FAIL** (env case currently refused).

- [ ] **Step 3: Add the `script` membership set + `is_script`**

In `Walk` (validate.rs ~607) add `script: HashSet<(&'a str, &'a str)>`. In the constructor (~790) populate it like `log` is populated (filter tasks whose `known_type(&t.r#type)` resolves to the Script handler). The six script FQCNs are: `egret.container`, `egret.script.python`, `io.kestra.plugin.scripts.python.Commands`, `io.kestra.plugin.scripts.python.Script`, `io.kestra.plugin.scripts.shell.Commands`, `io.kestra.plugin.scripts.shell.Script` — if the registry exposes a canonical Script `TYPE_ID`, filter on that (mirror the `log::TYPE_ID` filter at ~786); otherwise match `t.r#type` against those six strings. Compute near `is_log` (~683):
```rust
let is_script = matches!((id, ty), (Some(i), Some(t)) if self.script.contains(&(i, t)));
```

- [ ] **Step 4: Add the env allow-arm**

In the `else if` chain (after the `is_log && k == "message"` arm, ~709) add:
```rust
} else if is_script && k == "env" {
    // Package 10b: a script task's `env:` values resolve `secret()`
    // (resolved async in the handler, injected via a secure channel).
    // A `namespace=` / `full=` in the call is still refused, as elsewhere.
    let mut found = Vec::new();
    strings(v, here, &mut found);
    for (p, text) in found {
        self.property(p, text);
    }
}
```
Every other script field still falls through to `self.value(...)` → `outside` → refused.

- [ ] **Step 5: Update `SECRET_REFUSAL`** (functions.rs ~325) to include script env and redirect authors:
```rust
pub(crate) const SECRET_REFUSAL: &str = "The 'secret' function resolves only on `kestra.*` \
     plugin tasks, the http tasks, a Log task's `message:`, a task's `runIf:`, and a script \
     task's `env:`. It is refused here: on a script task the rendered commands, script and \
     inputFiles are fields of the pod spec (or land on disk), readable by anyone with \
     `get pods` in the task namespace -- put the secret in `env:` and reference it as an \
     environment variable (e.g. `$NAME`) in your command instead.";
```
Update the doc comment above it (~320) to match.

- [ ] **Step 6: Run gate + validate tests — expect PASS**
```bash
cargo test -p egret-workflow --test plugins_gate --test routes --test execution_timestamps
cargo test -p egret-workflow --lib spec::validate
```

- [ ] **Step 7: Commit**
```bash
git add crates/egret-workflow/src/spec/validate.rs crates/egret-workflow/src/runners/functions.rs crates/egret-workflow/tests/plugins_gate.rs
git commit -m "feat(workflow): lift secret() in script env at save; keep other fields refused"
```

---

### Task 6: Docs (repo `egret-data/docs`)

**Files (in the docs repo checkout `/Users/1pro-l4r8/Documents/github/data-portal/docs`, new branch `docs/workflow-secret-in-script-env`):**
- Modify: `platform/workflows/flows/expressions.md` — "Where secret() works": add script task `env:`; show the `$NAME` pattern; `:::caution` that on docker the value is visible via `docker inspect`.
- Modify: the deployment/RBAC doc — the engine k8s service account needs `create` on Secrets in task namespaces (add the verb to the RBAC snippet; explain it is used for secret() env injection).
- Modify: `platform/.../known-issues.md` — the docker `inspect` residual exposure (if not fully covered by the caution).

- [ ] **Step 1:** Make the edits above (bilingual per the docs repo's convention — update both language trees if the docs repo is bilingual).
- [ ] **Step 2:** Verify the docs build gate locally if available; otherwise rely on the docs PR's `build` check.
- [ ] **Step 3: Commit**
```bash
git add platform/workflows/flows/expressions.md <deployment-doc> <known-issues>
git commit -m "docs(workflow): secret() in script env — usage, docker caution, RBAC"
```

---

## Self-Review

- **Spec coverage:** D1 posture → Tasks 3/4; D2 env-only → Task 2 classify + Task 5 arm; D3 k8s secretKeyRef+ownerGC → Task 3; D4 docker env-file+unlink → Task 4; D5 masking → Task 1 plumbing + Task 2 wiring; D6 RBAC → Task 3 error + Task 6 docs; §4 outage fail-safe → Task 2 `Err` arm; §4 replay → Task 3 deterministic name; §5 Task 0 census → Task 0; tests → every task; docs → Task 6. No gap.
- **Placeholder scan:** every code step carries concrete code; the two located research pointers (script membership set construction; reuse of the 10a test harness) name the exact file, the mirror site, and a fallback — not "TBD".
- **Type consistency:** `secret_env: BTreeMap<String,String>`, `secret_masks: Vec<String>` used identically in Tasks 1–4; `render_with_secrets`/`current`/`text_uses_secret`/`Resolved.values` signatures match secrets.rs; `stream_line(log: &RunLog, …)` consistent across Tasks 1 and its callers.
