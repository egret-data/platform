# KI-325, KI-332, KI-343: policy-profile concurrency and compensation

## KI-332 deadlock moving an engine between profiles

`PUT /engines/{id}` with another `authzServiceId` ran `UPDATE engine_mgmt.engines`,
whose binding trigger (migration 0187) bumps the old and the new profile in one
statement, in no fixed order. `PATCH …/external` already locks both first
(`profile_compiler::lock_profiles`, sorted ascending, #813). Decision: the PUT
path (`apply_authz_service_update`) does the same before its snapshot and write.
No migration. Test: a recording compiler asserts `lock_profiles_in_tx` receives
the old and the new profile on a move (red before: never called).

## KI-325 tag-service change races an engine bind

The tag-service route checked "does an engine download this profile" through the
engine reader BEFORE its transaction. Decision: the check moves into the
transaction, as a direct read of `engine_mgmt.engines`, after the profile and the
tag services it touches are locked in ascending id order
(`lock_tag_service_change_via`; the bind locks the profile's expanded set
ascending too, so the two cannot take a profile and its lower-id tag service in
opposite orders). The bind paths (PUT, PATCH external, external register) re-run
`check_profile_bindable` after taking the profile locks, so a change that
committed first is seen and a change that queues behind the bind finds the engine
bound (409). Test: a bind transaction holds the profile lock with an uncommitted
engine row; the change queues on the lock (polled through `pg_stat_activity`, no
sleep race), the bind commits, the change must answer 409 and write nothing.

## KI-343 add-engine failure leaves a Trino profile

Create compensation deleted the profile it made, but the first compile had
already written a compiler-owned policy into it, which `delete_service` refuses.
Decision: after the engine row is gone, the compensation recompiles each profile
it is about to delete (the compiler reaps policies nothing produces any more),
then deletes it.
