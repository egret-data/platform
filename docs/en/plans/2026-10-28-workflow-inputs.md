# Workflow package #3 "Inputs checked at start" — implementation plan

Spec: `docs/en/specs/2026-10-28-workflow-inputs-design.md` (decisions D1–D16). Survey: `docs/en/specs/2026-10-28-workflow-inputs-survey.md`. Branch `feat/workflow-inputs`. Crate `crates/egret-workflow` unless stated; paths relative to `src/`.

## Global constraints

- The spec's D-numbers are binding; where this plan and the spec differ, the spec governs. Kestra source: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` (survey cites file:line).
- Error texts are Kestra's verbatim (spec D3). Status 400, body `{"error": …}`.
- Conventional Commits; trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; no network (the controller pushes); never touch ports 5432 or `egret-dev-*`/`egret-demo-*` containers; scratch never into git.
- Any new `std::env::var` needs `cargo test -p egret-config`. Oracle ratchet: `cargo test -p egret-workflow --test executor` must not regress (301 / 52 / 52 / 0 at the start).
- Test infra: private Postgres + Valkey containers named `egret-inputs-postgres` (:5461) and `egret-inputs-valkey` (:6461); `CARGO_TARGET_DIR=<worktree>/target`.

## Part A — PR #3a (backend)

**A1. Model.** `models/input.rs`: add flat fields `validator`, `min`, `max` (JSON numbers / ISO duration strings kept as `serde_json::Value` or typed per D4 — pick the least invasive that round-trips), `after`, `before`, `values` (scalars or `{label,value}`), `expression`, `allow_custom_value` (default false), `is_radio`, `auto_select_first`, `json_schema`. camelCase serde; round-trip tests in the style of the existing ones. Remove the `not_honoured` warnings for the keys now modelled (`spec/keys.rs:192-204`, `:881-915`) and update `spec/schema/property-treatment.json`; FORM `inputs` / REUSABLE `ref` stay warned. Check the schema README and the generated JSON schema stay consistent.

**A2. Scalar checker.** New `inputs_check.rs` (pure, `#![forbid(unsafe_code)]` style as the crate): `resolve(inputs: &[Input], posted: &Value) -> Result<Value, InputError>`; scalar types, required, empty-string rule (D5), STRING/SECRET `validator` (`fancy-regex`, add as a direct dependency), INT/FLOAT/BOOL/DATETIME/DATE/TIME/DURATION parse + bounds, URI, EMAIL, SELECT `values` (D4 quirk). Table tests per type: valid, every error text, empty string, null, JSON number vs string.

**A3. Collections, dependsOn, expression.** ARRAY/MULTISELECT (+ `itemType`), JSON/YAML/ION parse, `dependsOn.inputs` and `condition` (render via the workflow renderer with the dependency values; locate its entry point first), SELECT/MULTISELECT `expression`, `autoSelectFirst`. Decide `jsonSchema` (D12) by checking for an existing JSON Schema crate in `Cargo.lock`; if none is acceptable, leave the warning and record it.

**A4. Wire in.** Call `inputs_check::resolve` in `worker/claim.rs` `start_execution_in_on` between the object normalisation and `secret_inputs::split`; store the coerced object (D2); skip for `replayed_from_execution_id` and for Flow-trigger starts (D7) — find how a Flow-trigger start is identified in `NewExecution` and add an explicit flag if there is none. Subflow: call the same function in `worker/subflow.rs` before `secret_inputs::split` (:497), failing the parent run with `subflow inputs: …`. Map the error to `Error::InvalidInput`. Update the 400 text in the `start_execution` utoipa docs (`routes.rs` ~2541) and regenerate `openapi.json` (`cargo run -p egret-bin --bin egret -- openapi`) and the client (`npm --prefix frontend run gen:api`). Integration tests (`tests/inputs_start.rs`): API JSON, multipart, webhook, schedule, subflow child, replay not re-checked, Flow trigger not checked and born-FAILED fire still works, SECRET validator sees the clear value, nothing is created on refusal.

**A5. Save-time gaps (D11).** In `spec/validate.rs` next to `input_item_type_invalid`: unknown `dependsOn` input, cycle, `defaults` + `prefill`, FILE `defaults` scheme, uncompilable `validator`. Messages as Kestra's (survey §1.1). Measure whether `fancy-regex` rejects `\` like Java for `inputs-bad-validator-syntax`; report the result either way.

**A6. Harness seam + bank.** `tests/executor.rs` and `oracle-exec/driver.rs:1175`: add the start seam (D8); bank `invalids/inputs-with-multiple-constraint-violations`; bank `inputs-bad-validator-syntax` only if A5 measured it. Update `oracle-exec/baseline.txt` and README. Measured ratchet goes in the spec.

**A7. Docs and gates.** VI mirrors of the spec and this plan, roadmap row #3 (EN+VI), the oracle README, spec status line. Gates: `cargo test -p egret-config -p egret-server`, the full `egret-workflow` suite (only the 7 `sql_starrocks_live` may fail), `cargo fmt --check`, `cargo clippy` as CI does.

## Part B — PR #3b (validate endpoint and run form)

**B1. Endpoint (D14).** `POST …/flows/{id}/executions/validate`, JSON body, always 200 with Kestra's shape, per-input errors via a `resolve_all` variant of the checker that collects every input's error instead of stopping. Nothing stored or uploaded. 404/409 as the start. utoipa docs, `openapi.json`, client regeneration, drift tests. Route tests (200 on invalid, all inputs listed, no execution row).

**B2. Run form (D15).** `frontend/src/components/workflows/run/inputModel.ts`, `RunFlow.tsx`: new client checks, call the validate endpoint on submit, attribute errors per input, rewrite the stale module comment, fix `BackfillDialog.tsx` `"BOOLEAN"` → `"BOOL"`. Vitest for each. Needs `npm --prefix frontend ci` in the worktree.

**B3. Docs.** Notes for `egret-data/docs` (user-facing: what is checked at start, messages, 400, the validate endpoint, divergences BOOL / NaN / webhook status), roadmap and spec status.
