# KI-361: ep_gate shared-DB flake

`an_ep_with_legacy_inputs_is_incomplete_then_complete_after_migration` compared
two READ COMMITTED reads (`ep_gate_via` on a pooled connection, then
`ep_gate_via_tx`). The verdict includes installation-wide refusals
(`ClearanceTags` from unscoped `perm.rules`), which other tests in the shared
test DB insert/delete between the two reads, so the equality failed spuriously.

Fix (test-only): the equality check computes both paths inside one REPEATABLE
READ transaction, so a single snapshot backs both. The earlier per-EP asserts
are unchanged. No product code, migration or API change.
