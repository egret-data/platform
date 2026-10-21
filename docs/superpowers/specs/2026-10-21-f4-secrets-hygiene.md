# F4: Secrets hygiene (KI-112, KI-146, KI-152)

Package F4 of the known-issues fix programme
(`docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`).

## KI-112 (S1): workflow SECRET inputs

**Premise, re-verified on main.** `InputType::Secret` was read by nothing
but the validator. A posted value went verbatim into
`workflow.executions.inputs` (jsonb), which `ExecutionRepo`'s projection
returns on every read; a `Log` that printed it stored it in
`workflow.task_logs`. Reproduced by `tests/secret_inputs.rs` on the old code.

**Design: mask in the row everyone reads, seal beside it.**

- Migration 0203 adds `workflow.executions.secret_inputs bytea`: the SECRET
  values as one JSON object, sealed with `egret_secrets::Crypto` (AES-256-GCM
  under `secrets.master_key`, the workspace secret store's key), stored
  `nonce || ciphertext`. `inputs` holds `******` for each SECRET input.
- `egret_workflow::secret_inputs` owns it: `split` (mask + seal, at every
  execution-creating INSERT that takes posted or rendered inputs: the start,
  which every API/webhook/schedule/Flow-trigger/backfill start goes through,
  and a `Subflow` child against the child's declaration), `reveal` (only the
  render surface, `worker::render::Surface::load`). The cipher is installed
  process-wide by egret-bin, the pattern `storage::install` set.
- Copies: a `Loop` iteration (`worker::spawn`) and a `CREATE_NEW_EXECUTION`
  retry (`worker::replay`) copy the column with `inputs`; the replay route
  passes the source's sealed bytes (`NewExecution::secret_inputs`) so the
  replay runs with the stored secret, not the mask. A resume is the same row.
- The run log: every line of a claimed run is masked with the execution's
  SECRET values (`RenderContext::redactions` -> `RunLog::masked`), and so is a
  failed run's message (`task_runs.message`).
- No master key: a start that would store a SECRET value is refused (400,
  naming the input and `secrets.master_key`), as Kestra refuses one without
  encryption configured. Nothing is stored in clear.
- Existing rows: SQL cannot tell which inputs are SECRET (that is the YAML in
  `source`), so egret-bin runs `secret_inputs::backfill` at startup: every
  execution whose source declares a SECRET input and whose `secret_inputs` is
  NULL gets its values masked and, with a key, sealed. Idempotent.

**Read paths checked by the tests** (all through the real routes): the
start's answer; `GET .../executions/{id}`; `GET .../executions` and
`?flow_id=`; `GET .../executions/{id}/graph`; `GET .../executions/{id}/logs`;
the answers of replay and resume; and the stored `inputs`, `secret_inputs`,
`task_runs.message`, `task_logs.message`. The run's proof that it saw the
real value: the `Log` writes `len={{ inputs.pw | length }}`.

**Not masked, deliberately:** a task's `outputs` (a `Return` of the input
returns it), as `secret()` values are not (W5b spec section 12, as Kestra):
the author published it. Also not masked: `task_runs.rendered` (a flowable's
rendered `values:`/condition), a Flow trigger's or a Subflow's rendering of
the value into a NON-SECRET input of another flow, and a webhook's trigger
record. Each is the flow author writing the value somewhere on purpose.

## KI-146 (S3): `egret init` echoes a supplied password

Reproduced by reading `init` and by `crates/egret-bin/tests/init_output.rs`
on the old code. `bootstrap::AdminPassword` says where the password came from;
a supplied one prints `taken from EGRET_ADMIN_PASSWORD (not shown)`, a
generated one prints once. An empty variable now generates instead of
creating an admin with an empty password.

## KI-152 (S3): Helm dev realm

Reproduced with `helm template --set oidc.enabled=true`. Picked the gate (the
smaller change): `oidc.devRealm` (default `false`); with `oidc.enabled` and no
opt-in the chart fails to render with a message naming the realm's settings;
with the opt-in, NOTES prints a warning. `just deploy-kind-oidc` and
`just deploy-kind-governance` opt in. The default render is unchanged; the
opt-in render equals the old `oidc.enabled` render (only the chart's random
generated secrets differ). No cluster was available to install on.
