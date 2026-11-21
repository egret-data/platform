# KI-272: engine settings save survives a dropped request

## Premise re-check (origin/main 28f41daa5)

KI-263 (#735) moved the provisioner call and its bookkeeping into
`finish_apply`, spawned by `apply_built_noting`. That covers every caller of
`apply_built` (catalog apply, restart-recreate, rotate secrets, rotate ranger
credential, security upgrade): none of them has a step after it that must run.

The two settings-save handlers do:

| Path | After the apply, in the request |
| --- | --- |
| `PUT /engines/{id}` Trino (`update_trino_engine`) | `restore_engine_spec` on a failed apply; `rematerialize_or_400` |
| `PUT /engines/{id}` StarRocks (`update_starrocks_engine`) | `restore_engine_spec` on a failed build or apply; `rematerialize_or_400`; also `update_starrocks_cluster` (the provisioner call itself) is not spawned |

So KI-272 reproduces on both. External engines have no apply (`external.rs`
calls neither).

## Fix

Run each handler's whole persist -> build -> apply -> restore/rematerialize
tail in a spawned task (`run_detached`), awaited by the request exactly like
`apply_built` awaits `finish_apply`. A request that is dropped no longer drops
the tail; a connected request gets the same result as before.

## Test

`a_dropped_update_request_still_restores_the_spec_after_a_failed_apply`: a
provisioner whose `update` blocks, the request future aborted while it is
blocked, then released to fail; the stored spec must come back.
