# F3: Workflow worker: parallel claims and visible "worker off"

Known-issues fix programme, package F3 (wave 1). Rows: KI-187 (S1), KI-013 (S1).

## Premise (re-verified on main `50235eed7`)

- **KI-187 reproduces.** Demo stack, admin: `f3.ki187.f3-slow` (one `Sleep` PT90S) started, then 4 s later
  `f3.ki187.f3-fast` (two `Log` tasks). The fast execution stayed `RUNNING` for 88 s and ended in the same poll as
  the Sleep. Cause: `runtime::spawn_worker`'s claim loop awaits `worker::tick_with`, which awaits `run_one` for each
  claimed run in turn, so the loop does not tick again until the slowest run of the batch commits.
- **KI-013 reproduces (by code and test; the demo stack runs the worker).** `workflow.worker_enabled` defaults to
  `false`; `runtime::spawn` then starts no claim loop (`spawn_with_the_worker_disabled_never_calls_the_registry`).
  `POST …/flows/{id}/executions` answers 201 with no hint, and `FlowDto`, `ExecutionPageDto` and `ExecutionDto` carry
  no flag. Only the trigger routes echo `TriggerFlags::worker_enabled`, and only the Triggers page shows a banner.
  The demo stack sets `EGRET_WORKFLOW__WORKER_ENABLED=true` (`deploy/docker-compose.demo.yml`) and keeps it.

## What changes

**KI-187.** New key `workflow.max_concurrent_runs` (default 8, `0` is a boot error,
`EGRET_WORKFLOW__MAX_CONCURRENT_RUNS`), passed as `RuntimeConfig::max_concurrent_runs`. The claim loop keeps a
`JoinSet` of runs in flight; each tick claims at most `min(free slots, batch)` runs with the new
`worker::start_claimed`, which spawns `run_one` per claimed run (unchanged: lease renewal, kill path, fenced commit)
and returns at once. So a slow run holds one slot, not the loop.

- *Decision:* claim only free slots instead of claiming a batch and queueing it. A claimed run therefore starts at
  once and its lease is renewed from the claim on; a run beyond the bound stays unclaimed for another replica. This
  also removes, on the runtime path, the "later member of a batch reaped before its handler starts" window that
  `tick`'s doc records.
- *Decision:* `tick`/`tick_with` stay sequential. They are the primitive the tests and the oracle driver call and
  whose ordering the W4 ruling documents; only the runtime's loop changes.
- Shutdown: on cancel the loop stops claiming and waits for every run in flight before it ends (and so before the
  log writer is released). A panicking run is logged; its lease lapses for the reaper.

**KI-013.** The one source stays `TriggerFlags::worker_enabled` (the config flag). It is now also echoed as
`worker_enabled` on `FlowDto`, `ExecutionPageDto` and `ExecutionDto` (so the Run answer, replay, restart and the
execution detail carry it). The UI shows one notice, `WorkerOffNotice`, with what to set
(`workflow.workerEnabled` / `EGRET_WORKFLOW__WORKER_ENABLED=true`) on the flow page, in the Run form, on the
executions list (not on the flow's own Executions tab, under the flow page's notice) and on an execution that has not
ended. The Triggers banner uses the same sentence.

## Tests

- `tests/runtime.rs`: `a_slow_task_does_not_hold_up_another_execution_on_the_replica`,
  `the_claim_loop_runs_at_most_max_concurrent_runs_at_once`, `a_cancel_waits_for_the_runs_in_flight_and_claims_nothing_new`
  (real claim loop, shipped `Sleep`/`Log` handlers).
- `tests/executions_routes.rs`: `the_run_answer_the_flow_and_the_executions_say_whether_the_worker_runs` (both values).
- `egret-config`: `validate_refuses_zero_concurrent_runs`.
- Frontend vitest for the notice on the flow page, Run form, executions list and execution detail.
