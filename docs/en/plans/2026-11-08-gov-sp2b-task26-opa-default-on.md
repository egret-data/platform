# Governance SP2b Task 26: OPA on by default (implementation plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** turn `[governance.opa] enabled` on by default, after closing the readiness items that remained once the port-8080 package (#842, #843) merged.

**Sources:**
- SP2b plan Task 26: `docs/en/plans/2026-11-04-gov-sp2b-opa-target.md`, section "Task 26: Turn OPA on by default".
- The final review of 2b-2, section "Task 26 readiness" (items 1–13). Items 1, 2, 3 and 6 are done: I2 and plain-HTTP identity by #842 and #843, I1/N2a by #839, and `opa-e2e` is green on `main` at 666135744.
- User decisions, 2026-11-07:
  - item 7 (no tag service): warn in the switch answer and in explain, no block;
  - item 8 (KI-352, write-only-deny listing): keep the behaviour, docs only;
  - items 9 and 10 (KI-353 lineage, KI-354 external own-profile): docs only, flip anyway.
- Controller decision, 2026-11-08 (Helm coherence): the chart's `opa.enabled` defaults to `true` as well, so a default install renders the OPA port, Service port and NetworkPolicy rule that a default-on backend needs.

**Branch:** `feat/opa-default-on`, worktree `egret-t26`. Docs go to `egret-data/docs`, branch `docs/opa-default-on`.

## Global Constraints

- Use your own target directory: `CARGO_TARGET_DIR=$PWD/target-t26`.
- DB tests run with `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.
- Never run two test suites at once.
- Never prune Docker, never touch containers you did not start, and never run `just up`, `just down` or `just reset-testdata`.
- Regenerate the frontend client only with `npm run gen:api`.
- Every behaviour change has a test that fails without it, and a mutant of the change that the test kills, recorded in the report.
- No migration is expected. If one is needed, take the next free number above `main` and every open PR.
- Commits use Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before finishing, `cargo fmt --all -- --check` and `cargo clippy` with `-D warnings` on the touched crates are clean.

---

### Task 1: A full-router test of a large `/batch` (readiness 5, M4)

**Why:** nothing tests a `/batch` body over 4 MiB through the router that production serves. A future outer body limit would refuse every `SHOW TABLES` on a large schema, and no test would fail.

**Files:** `crates/egret-server/tests/opa_surface_mounted.rs`, which already builds `egret_server::opa_router(&AppState)`.

**Requirements:**
- Through `opa_router`, which since #843 is the router the OPA listener serves:
  - a `/v1/data/egret/trino/batch` body of about 5 MiB, below the default `batch_body_limit_bytes` of 24 MiB, is read and answered. The answer is not 413 (an unauthenticated body may be refused with 403; what matters is that the limit did not refuse it);
  - a `/batch` body above the configured `batch_body_limit_bytes` (set the limit low in the test config, for example 4 MiB, and send 5 MiB) gets 413;
  - an `/allow` body over 4 MiB gets 413.
- Mutants: removing the per-route `/batch` limit, or adding an outer 4 MiB `RequestBodyLimitLayer` on `opa_router`, makes a test fail.

### Task 2: Warn when a Trino profile has no tag service (readiness 7)

**Why:** a Trino profile without a tag service silently ignores every classification rule and clearance, on both mechanisms. The user chose to warn, not block.

**Requirements:**
- **Switch answer.** `PATCH …/mechanism` (`crates/egret-policy/src/admin/mechanism.rs`, `ProfileMechanism`) gains an optional `no_tag_service_warning: Option<String>`. It is set when the profile is a Trino profile with no `tag_service`, on the 200 body and on the 502 body alike. Suggested text: "This profile has no tag service, so classification rules and clearances do not apply to its engines. Bind a tag service in the profile's settings."
- **Explain.** `ExplainProfile` (`crates/egret-perm-compile/src/routes/`) gains the same optional warning, set for a Trino profile with no tag service on both mechanisms, Ranger and OPA. Read the tag service in explain's own transaction, as the rest of explain reads profile state.
- **Frontend.** `MechanismSection` shows the switch warning after a successful or a 502 switch, next to `external_engines_warning`. `ExplainResult` shows the explain warning once per profile. Regenerate the client with `npm run gen:api`. Add vitest tests for both.
- **OpenAPI:** the utoipa descriptions name the new field.
- **Tests:**
  - DB tests: a switch of a tagless Trino profile carries the warning; a profile with a tag service does not;
  - explain: the same two cases, on Ranger and on OPA;
  - a mutant that inverts the condition fails.

### Task 3: Refresh latency with a shared tag service (readiness 4, M5)

**Why:** rebuilds are sequential, and each rebuild of a profile that shares a tag service loads the units of every profile sharing it (`crates/egret-perm-compile/src/opa/cache.rs` around 313–347, `overlap.rs` around 492–507). So a tag-version bump costs O(N²) across N profiles that share a tag service, and the last profile goes current last.

**Requirements:**
- **Measure first**, with the harness of section 4 of `docs/en/reports/2026-11-06-gov-sp2b-opa-latency.md` (`SnapshotCache::refresh_once` timings). Set up N profiles bound to one tag service, for N = 1, 5, 10 and 20, each with a realistic policy count (reuse the report's 984-policy profile). Bump the tag version, then record the time until the last profile serves a snapshot at the new key. Take 10 runs per N, and report p50, min and max.
- **Decide by the numbers:**
  - If the last profile at N = 10 becomes current within 1 s, change no code. Report the numbers.
  - Otherwise, make the rebuild of a profile load only `{p, tag(p)}`, or another change you justify, and measure again.
  - Either way, keep the C8 stored-state check: overlap across profiles that share a tag service is still refused. Show that a test covers it.
- **Stale-statistics window:** state how long, per rebuild, the surface keeps answering from the previous snapshot (22F.3 measured up to 810 ms per rebuild), and how that window scales with N.
- **Report:** append a dated section, "Shared tag service (Task 26)", to `docs/en/reports/2026-11-06-gov-sp2b-opa-latency.md`, with the raw output under that report's `RAW/` directory, and the same summary in the `docs/vi` mirror if one exists.

### Task 4: Turn OPA on by default (SP2b plan Task 26, readiness 13)

**Requirements:**
- `crates/egret-config/src/lib.rs`: `OpaSettings::enabled` defaults to `true`, via `#[serde(default = "default_true")]` and a `fn default_true() -> bool { true }` beside it (reuse an existing one if the file has it). Also update `impl Default for OpaSettings` if it exists. Doc comment: "On by default since SP2b-2; an operator can still turn it off, and then the surface is not mounted and a switch to OPA is refused".
- Rename `governance_opa_is_off_by_default` to `governance_opa_is_on_by_default`. Write the test change first and see it fail. The env spelling `EGRET_GOVERNANCE__OPA__ENABLED=false` turns it off: add an assertion.
- **Fallout:** every test that relied on "off by default" sets `enabled = false` explicitly. For example, `governance_opa_listen_is_not_checked_while_opa_is_off` and the egret-bin and egret-server tests that build a default config. Look for listener collisions: with OPA on, `0.0.0.0:8082` is bound at start-up, so a test that starts a server, and every compose file, must not already use 8082 for something else (`grep -rn 8082 deploy scripts crates`).
- **Helm** (`deploy/helm/egret`): `opa.enabled: true` in `values.yaml`, with the comment updated. Check that `configmap-env.yaml` renders `EGRET_GOVERNANCE__OPA__ENABLED` from it either way. If the env is rendered only under `opa.enabled`, render `"false"` explicitly when it is off, so the chart's value always wins over the binary's default. Update `crates/egret-server/tests/helm_opa_listener.rs` (or its equivalent) for the default-on render and for an explicit `--set opa.enabled=false` render.
- **Compose:** the dev and demo compose files keep working. If a compose file sets `EGRET_GOVERNANCE__OPA__ENABLED=true`, it may stay. Do not edit files under `egret-dev-*`, `egret-demo` or `example-dwh*`.
- **Run:** `cargo test -p egret-config governance`, `cargo test -p egret-server`, `cargo test -p egret-bin`, and the Helm test.

### Task 5: Docs (in `egret-data/docs`)

**Requirements, each on its existing page; find them with grep:**
- **"Trino on OPA"** (`platform/data-governance/engines-and-policy/trino-on-opa.md`): the prerequisites say the key defaults to `true` from this release, and how to turn it off (`[governance.opa] enabled = false`, env `EGRET_GOVERNANCE__OPA__ENABLED=false`, Helm `opa.enabled=false`).
- **Configuration keys:** the default changes to `true`.
- **Kubernetes:** `opa.enabled` defaults to `true`.
- **Release notes and upgrade:** OPA is on by default. Profiles stay on Ranger until an operator switches them, so the flip alone changes no enforcement. Port 8082 is now bound by default.
- **Known issues:**
  - KI-352: a write-only deny hides a readable table from listings on OPA;
  - KI-353: lineage EXPLAIN on OPA engines fails closed;
  - KI-354: an external Trino 483 on its own compiled profile cannot run any query.

  Check whether these entries exist already, and add or update them.
- **Tag-service warning:** document the new warning (Task 2) where the switch and explain are described.
- **Task 25 CARRY wording:** the "/opa/ network restriction REQUIRED" text must match the I2 resolution. The OPA surface is served only on its own listener (default 8082). The chart's NetworkPolicy admits only the engine namespace and `opa.extraFrom`, while `engineNetworkPolicy.enabled` is on. Find stale wording that says `/opa/` is on 8080 and fix it.
- **Shared-tag latency:** a short note on the Task 3 result, where latency or tag services are described.
- **The `trino-config` screenshot re-shoot** needs a running stack with the new image. It is a controller step, not part of this task.
- Every code fence carries a language. `npm run build` in the docs repo passes.
